//! Network mode (`--connect`): the chunk columns come from a server (`nascio-client`) and are
//! meshed from what it sent, without generating them here (S4b). Sky, climate and horizon
//! still come from the local world: the viewer is a development tool; the standalone client
//! will get them from the server (`docs/reports/serveur-S4a.md`).
//!
//! A thread runs the connection on its own tokio runtime and fills a [`Mirror`]; the columns
//! that arrive or change (and their neighbours, whose borders they are) are queued for
//! meshing again.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, ensure};
use bevy::prelude::{Local, Res, ResMut, Resource, Time};
use nascio_client::{Event, Mirror, connect};
use nascio_core::consts::CHUNKS_PER_AXIS;
use nascio_core::coords::ChunkPos;
use nascio_protocol::messages::{ClientMsg, ServerMsg};
use nascio_voxel::block::Blocks;
use nascio_voxel::{ChunkMesh, mesh_chunk};

use crate::fly::{FlyCam, Streamer};

/// The connection, shared with the workers.
#[derive(Resource, Clone)]
pub struct Remote {
    pub mirror: Arc<Mutex<Mirror>>,
    out: tokio::sync::mpsc::UnboundedSender<ClientMsg>,
    dirty: Arc<Mutex<BTreeSet<(i32, i32)>>>,
    /// Last event worth showing (refused edit, closed connection).
    pub status: Arc<Mutex<String>>,
    /// Radius of the columns the server sends, metres.
    pub radius_m: u32,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Remote {
    /// Connects (waits for the handshake) and starts the connection thread. `blocks`: the
    /// local block table, which must be the server's (same world).
    pub fn start(
        addr: SocketAddr,
        cert: &Path,
        blocks: &Blocks,
        spawn_pos: [f64; 3],
    ) -> anyhow::Result<Self> {
        let cert = std::fs::read(cert).with_context(|| format!("reading {}", cert.display()))?;
        let names: Vec<String> = (0..blocks.len())
            .map(|i| blocks.def(i as u16).name.clone())
            .collect();
        let mirror = Arc::new(Mutex::new(Mirror::default()));
        let dirty = Arc::new(Mutex::new(BTreeSet::new()));
        let status = Arc::new(Mutex::new(String::new()));
        let (out, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<ClientMsg>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<u32>>();
        let (m, d, st) = (mirror.clone(), dirty.clone(), status.clone());
        std::thread::Builder::new()
            .name("network".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => return drop(ready_tx.send(Err(e.into()))),
                };
                rt.block_on(async move {
                    let mut c = match connect(addr, &cert, "viewer").await {
                        Ok(c) => c,
                        Err(e) => return drop(ready_tx.send(Err(e))),
                    };
                    if c.hello.blocks != names {
                        let _ = ready_tx.send(Err(anyhow::anyhow!(
                            "the server's block table differs from this world's"
                        )));
                        return;
                    }
                    let _ = ready_tx.send(Ok(c.hello.chunk_radius_m));
                    c.send(ClientMsg::Move {
                        pos: spawn_pos,
                        yaw: 0.0,
                        pitch: 0.0,
                    });
                    let mark = |cx: i32, cz: i32| {
                        let mut d = lock(&d);
                        for dz in -1..=1 {
                            for dx in -1..=1 {
                                d.insert((
                                    (cx + dx).rem_euclid(CHUNKS_PER_AXIS),
                                    (cz + dz).rem_euclid(CHUNKS_PER_AXIS),
                                ));
                            }
                        }
                    };
                    loop {
                        tokio::select! {
                            Some(msg) = out_rx.recv() => c.send(msg),
                            ev = c.events.recv() => match ev {
                                Some(Event::Column(col)) => {
                                    let (cx, cz) = (col.cx, col.cz);
                                    lock(&m).column(*col);
                                    mark(cx, cz);
                                }
                                Some(Event::Msg(ServerMsg::Blocks { chunk, lsn, changes })) => {
                                    lock(&m).blocks(chunk, lsn, changes);
                                    mark(chunk[0], chunk[2]);
                                }
                                Some(Event::Msg(ServerMsg::Forget { columns })) => {
                                    lock(&m).forget(&columns);
                                }
                                Some(Event::Msg(ServerMsg::EditRefused { pos, reason })) => {
                                    *lock(&st) = format!("edit at {pos:?} refused: {reason}");
                                }
                                Some(Event::Msg(ServerMsg::Kick { reason })) | Some(Event::Closed(reason)) => {
                                    *lock(&st) = format!("disconnected: {reason}");
                                    break;
                                }
                                Some(Event::Msg(_)) => {}
                                None => break,
                            },
                        }
                    }
                });
            })?;
        let radius_m = ready_rx
            .recv_timeout(Duration::from_secs(20))
            .context("no answer from the server")??;
        ensure!(radius_m >= 64, "server radius {radius_m} m too small");
        Ok(Self {
            mirror,
            out,
            dirty,
            status,
            radius_m,
        })
    }

    pub fn send(&self, m: ClientMsg) {
        let _ = self.out.send(m);
    }

    /// Whether column `key` and its neighbours are held (it can be meshed).
    pub fn can_mesh(&self, key: (i32, i32)) -> bool {
        lock(&self.mirror).can_mesh(key.0, key.1)
    }

    /// Meshes a column from the mirror: `None` while a neighbour is missing.
    pub fn mesh_column(&self, key: (i32, i32), blocks: &Blocks) -> Option<Vec<(i32, ChunkMesh)>> {
        let (padded, tint) = {
            let m = lock(&self.mirror);
            if !m.can_mesh(key.0, key.1) {
                return None;
            }
            let col = m.get(key.0, key.1)?;
            let top = col.cy_min + col.chunks.len() as i32;
            let padded: Vec<_> = (col.cy_min..top)
                .filter_map(|cy| Some((cy, m.padded_chunk(ChunkPos::new(key.0, cy, key.1))?)))
                .collect();
            (padded, col.tint)
        };
        Some(
            padded
                .into_iter()
                .map(|(cy, p)| (cy, mesh_chunk(&p, blocks, &tint)))
                .filter(|(_, m)| !m.is_empty())
                .collect(),
        )
    }
}

/// Columns that arrived or changed are meshed again; the camera position goes to the server
/// four times a second (it centres the columns sent).
pub fn remote_sync(
    remote: Option<Res<Remote>>,
    mut streamer: ResMut<Streamer>,
    cam: Res<FlyCam>,
    time: Res<Time>,
    mut since: Local<f32>,
) {
    let Some(remote) = remote else { return };
    let dirty: Vec<(i32, i32)> = std::mem::take(&mut *lock(&remote.dirty))
        .into_iter()
        .collect();
    if !dirty.is_empty() {
        streamer.request_remesh(dirty);
    }
    *since += time.delta_secs();
    if *since >= 0.25 {
        *since = 0.0;
        remote.send(ClientMsg::Move {
            pos: [cam.pos.x, cam.pos.y, cam.pos.z],
            yaw: cam.yaw as f32,
            pitch: cam.pitch as f32,
        });
    }
}
