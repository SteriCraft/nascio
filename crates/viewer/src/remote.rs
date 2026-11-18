//! Network mode (`--connect`): the chunk columns come from a server (`nascio-client`) and are
//! meshed from what it sent, with the block table of its hello, without generating them here
//! (S4b, chantier 1). This module uses only `nascio-chunk`, `nascio-client` and
//! `nascio-protocol`, like the future client. Sky, climate and horizon
//! still come from the local world: the viewer is a development tool; the standalone client
//! will get them from the server (`docs/reports/serveur-S4a.md`).
//!
//! A thread runs the connection on its own tokio runtime and fills a [`Mirror`]; the columns
//! that arrive or change (and their neighbours, whose borders they are) are queued for
//! meshing again. The other players come as entity datagrams ([`Entities`], acknowledged at
//! once) and are drawn as boxes at their interpolated positions (S5).

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use bevy::prelude::{
    Assets, Color, Commands, Component, Cuboid, Entity, Handle, LinearRgba, Local, Mesh, Mesh3d,
    MeshMaterial3d, Quat, Query, Res, ResMut, Resource, StandardMaterial, Time, Transform, Vec3,
};
use nascio_chunk::{Blocks, ChunkMesh, mesh_chunk};
use nascio_client::{Entities, Event, Mirror, connect};
use nascio_core::consts::CHUNKS_PER_AXIS;
use nascio_core::coords::{ChunkPos, WorldPos};
use nascio_protocol::messages::{ClientMsg, ServerMsg};

use crate::fly::{FlyCam, RenderOrigin, Streamer};

/// The connection, shared with the workers.
#[derive(Resource, Clone)]
pub struct Remote {
    pub mirror: Arc<Mutex<Mirror>>,
    out: tokio::sync::mpsc::UnboundedSender<ClientMsg>,
    dirty: Arc<Mutex<BTreeSet<(i32, i32)>>>,
    /// Last event worth showing (refused edit, closed connection).
    pub status: Arc<Mutex<String>>,
    /// The other players, and the local time base of their samples.
    pub entities: Arc<Mutex<Entities>>,
    /// Position the server put the camera back to (refused move).
    correction: Arc<Mutex<Option<[f64; 3]>>>,
    epoch: Instant,
    /// Radius of the columns the server sends, metres.
    pub radius_m: u32,
    /// The server's block table (meshing).
    pub blocks: Arc<Blocks>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Remote {
    /// Connects (waits for the handshake) and starts the connection thread. `local`: the
    /// block table of the local world, which must be the server's: the viewer still draws
    /// the horizon and picks the blocks of its edits from the local world.
    pub fn start(
        addr: SocketAddr,
        cert: &Path,
        name: &str,
        secret: &str,
        local: &Blocks,
        spawn_pos: [f64; 3],
    ) -> anyhow::Result<Self> {
        let cert = std::fs::read(cert).with_context(|| format!("reading {}", cert.display()))?;
        let local = local.registry().clone();
        let mirror = Arc::new(Mutex::new(Mirror::default()));
        let dirty = Arc::new(Mutex::new(BTreeSet::new()));
        let status = Arc::new(Mutex::new(String::new()));
        let entities = Arc::new(Mutex::new(Entities::new(0.05)));
        let correction = Arc::new(Mutex::new(None));
        let (name, secret) = (name.to_owned(), secret.to_owned());
        let epoch = Instant::now();
        let (out, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<ClientMsg>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<(u32, Blocks)>>();
        let (m, d, st, en) = (
            mirror.clone(),
            dirty.clone(),
            status.clone(),
            entities.clone(),
        );
        let co = correction.clone();
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
                    let mut c = match connect(addr, &cert, &name, &secret).await {
                        Ok(c) => c,
                        Err(e) => return drop(ready_tx.send(Err(e))),
                    };
                    if c.blocks.registry() != &local {
                        let _ = ready_tx.send(Err(anyhow::anyhow!(
                            "the server's block table differs from this world's"
                        )));
                        return;
                    }
                    let _ = ready_tx.send(Ok((c.hello.chunk_radius_m, c.blocks.clone())));
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
                                Some(Event::Entities(bytes)) => {
                                    let now = epoch.elapsed().as_secs_f64();
                                    match lock(&en).receive(&bytes, now) {
                                        Ok(seq) => c.send(ClientMsg::Ack { seqs: vec![seq] }),
                                        Err(e) => *lock(&st) = format!("entities: {e:#}"),
                                    }
                                }
                                Some(Event::Msg(ServerMsg::Correct { pos })) => {
                                    *lock(&co) = Some(pos);
                                    *lock(&st) = "move refused: too fast".into();
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
        let (radius_m, blocks) = ready_rx
            .recv_timeout(Duration::from_secs(20))
            .context("no answer from the server")??;
        ensure!(radius_m >= 64, "server radius {radius_m} m too small");
        Ok(Self {
            mirror,
            out,
            dirty,
            status,
            entities,
            correction,
            epoch,
            radius_m,
            blocks: Arc::new(blocks),
        })
    }

    pub fn send(&self, m: ClientMsg) {
        let _ = self.out.send(m);
    }

    /// Whether column `key` and its neighbours are held (it can be meshed).
    pub fn can_mesh(&self, key: (i32, i32)) -> bool {
        lock(&self.mirror).can_mesh(key.0, key.1)
    }

    /// Meshes a column from the mirror with the server's block table: `None` while a
    /// neighbour is missing.
    pub fn mesh_column(&self, key: (i32, i32)) -> Option<Vec<(i32, ChunkMesh)>> {
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
                .map(|(cy, p)| (cy, mesh_chunk(&p, &self.blocks, &tint)))
                .filter(|(_, m)| !m.is_empty())
                .collect(),
        )
    }
}

/// Columns that arrived or changed are meshed again; the camera position goes to the server
/// 20 times a second (it centres the columns sent and is what the others see).
pub fn remote_sync(
    remote: Option<Res<Remote>>,
    mut streamer: ResMut<Streamer>,
    mut cam: ResMut<FlyCam>,
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
    if let Some(p) = lock(&remote.correction).take() {
        cam.pos = WorldPos::new(p[0], p[1], p[2]);
    }
    *since += time.delta_secs();
    if *since >= 0.05 {
        *since = 0.0;
        remote.send(ClientMsg::Move {
            pos: [cam.pos.x, cam.pos.y, cam.pos.z],
            yaw: cam.yaw as f32,
            pitch: cam.pitch as f32,
        });
    }
}

/// A box standing for another player.
#[derive(Component)]
pub struct RemotePlayer(u32);

/// Mesh and material of the boxes.
#[derive(Default)]
pub struct PlayerLook(Option<(Handle<Mesh>, Handle<StandardMaterial>)>);

/// Places the other players at their interpolated positions (spawns and removes boxes).
pub fn draw_players(
    mut commands: Commands,
    remote: Option<Res<Remote>>,
    origin: Res<RenderOrigin>,
    mut boxes: Query<(Entity, &RemotePlayer, &mut Transform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut look: Local<PlayerLook>,
) {
    let Some(remote) = remote else { return };
    let now = remote.epoch.elapsed().as_secs_f64();
    let poses = lock(&remote.entities).poses(now);
    let (mesh, material) = look
        .0
        .get_or_insert_with(|| {
            (
                meshes.add(Cuboid::new(0.6, 1.8, 0.6)),
                materials.add(StandardMaterial {
                    base_color: Color::srgb(0.9, 0.2, 0.1),
                    emissive: LinearRgba::rgb(0.6, 0.1, 0.05),
                    ..Default::default()
                }),
            )
        })
        .clone();
    let o = WorldPos::new(
        f64::from(origin.0.x),
        f64::from(origin.0.y),
        f64::from(origin.0.z),
    );
    let place = |p: &nascio_client::Pose| {
        // The position is the eye; the box stands below it.
        let rel = WorldPos::new(p.pos[0], p.pos[1] - 0.7, p.pos[2]).relative_to(o);
        Transform::from_translation(Vec3::from_array(rel))
            .with_rotation(Quat::from_rotation_y(-p.yaw))
    };
    let mut seen = BTreeSet::new();
    for (e, rp, mut t) in &mut boxes {
        match poses.iter().find(|p| p.id == rp.0) {
            Some(p) => {
                *t = place(p);
                seen.insert(rp.0);
            }
            None => commands.entity(e).despawn(),
        }
    }
    for p in poses.iter().filter(|p| !seen.contains(&p.id)) {
        commands.spawn((
            RemotePlayer(p.id),
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            place(p),
        ));
    }
}
