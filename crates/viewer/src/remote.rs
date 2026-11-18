//! Network mode (`--connect`): the chunk columns come from a server (`nascio-client`) and are
//! meshed from what it sent, with the block table of its hello, without generating them here
//! (S4b, chantier 1). This module uses only `nascio-chunk`, `nascio-client` and
//! `nascio-protocol`. The sky, the climate and the far ground come from the server's
//! `WorldSky`, the horizon from its tiles (chantier 4); both are kept in the disk cache
//! `client-cache/g<generator>-<fingerprint>/` next to the executable: the client reads no
//! world of its own.
//!
//! A thread runs the connection on its own tokio runtime and fills a [`Mirror`]; the columns
//! that arrive or change (and their neighbours, whose borders they are) are queued for
//! meshing again. The other players come as entity datagrams ([`Entities`], acknowledged at
//! once) and are drawn as boxes at their interpolated positions (S5). The player's own
//! movement is predicted by a [`Player`] shared with the frame ([`crate::player`]): this thread
//! gives it the server's states, death and respawn, and the round trip time (chantier 5.5).

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use bevy::prelude::{
    Assets, Color, Commands, Component, Cuboid, Entity, Handle, LinearRgba, Local, Mesh, Mesh3d,
    MeshMaterial3d, Quat, Query, Res, ResMut, Resource, StandardMaterial, Transform, Vec3,
};
use nascio_chunk::horizon::{HorizonTile, TileKey};
use nascio_chunk::{Blocks, ChunkMesh, mesh_chunk};
use nascio_client::{Auth, Entities, Event, Mirror, Player, connect_with};
use nascio_core::consts::CHUNKS_PER_AXIS;
use nascio_core::coords::{BlockPos, ChunkPos, WorldPos};
use nascio_physics::Shapes;
use nascio_protocol::messages::Chat;
use nascio_protocol::messages::{ClientMsg, ServerMsg};
use nascio_protocol::player::Commands as PlayerCommands;
use nascio_protocol::sky::WorldSky;

use crate::fly::{RenderOrigin, Streamer};

/// What the Bevy side hands to the network thread.
enum Out {
    Msg(ClientMsg),
    Commands(PlayerCommands),
}

/// The connection, shared with the workers.
#[derive(Resource, Clone)]
pub struct Remote {
    pub mirror: Arc<Mutex<Mirror>>,
    /// The player, predicted. Lock it before the mirror, never the other way.
    player: Arc<Mutex<Player>>,
    out: tokio::sync::mpsc::UnboundedSender<Out>,
    dirty: Arc<Mutex<BTreeSet<(i32, i32)>>>,
    /// Last event worth showing (refused edit, closed connection).
    pub status: Arc<Mutex<String>>,
    /// The other players, and the local time base of their samples.
    pub entities: Arc<Mutex<Entities>>,
    /// Chat lines received and not yet shown.
    pub chat: Arc<Mutex<Vec<Chat>>>,
    /// Why the connection ended (`None` while it lives).
    pub closed: Arc<Mutex<Option<String>>>,
    /// The player's spawn position from the server (world metres).
    pub spawn: [f64; 3],
    epoch: Instant,
    /// Radius of the columns the server sends, metres.
    pub radius_m: u32,
    /// The server's block table (meshing).
    pub blocks: Arc<Blocks>,
    /// The sky and far ground of the server's world.
    pub sky: Arc<WorldSky>,
    /// Horizon tiles that arrived, not yet taken by the tile store.
    pub tiles: Arc<Mutex<Vec<HorizonTile>>>,
    /// Disk cache of this world (`None`: no cache).
    pub cache: Option<std::path::PathBuf>,
}

/// Deepest search for the floor under the camera, blocks.
const FLOOR_SEARCH: i32 = 256;

/// `client-cache/` next to the executable.
fn cache_root() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default()
        .join("client-cache")
}

/// Cache folder of a world under `root`: `g<generator_version>-<fingerprint>`. `None` if the
/// server's fingerprint is not plain hexadecimal (it becomes a path).
fn cache_dir(
    root: &std::path::Path,
    generator_version: u32,
    fingerprint: &str,
) -> Option<std::path::PathBuf> {
    let ok = !fingerprint.is_empty()
        && fingerprint.len() <= 128
        && fingerprint.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| root.join(format!("g{generator_version}-{fingerprint}")))
}

/// The `WorldSky` file of a cache folder.
fn sky_file(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(format!("sky-p{}.bin", nascio_protocol::PROTOCOL_VERSION))
}

/// The file of a horizon tile in a cache folder.
pub fn tile_file(dir: &std::path::Path, k: TileKey) -> std::path::PathBuf {
    dir.join(format!("tiles-f{}", nascio_protocol::horizon::TILE_FORMAT))
        .join(format!("{}_{}_{}.bin", k.level, k.i, k.j))
}

/// Writes a cache file (its folder created); a failure only costs a download next time.
fn write_cache(path: &std::path::Path, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, bytes) {
        bevy::log::warn!("cache {}: {e}", path.display());
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Remote {
    /// Connects, gets the sky (disk cache or server) and starts the connection thread.
    /// `cert`: the server's certificate (DER), pinned. The player starts at the spawn point of
    /// the server.
    pub fn start(addr: SocketAddr, cert: Vec<u8>, name: &str, auth: Auth) -> anyhow::Result<Self> {
        Self::start_in(addr, cert, name, auth, cache_root())
    }

    /// [`Remote::start`] with the disk cache under `root` (tests).
    pub(crate) fn start_in(
        addr: SocketAddr,
        cert: Vec<u8>,
        name: &str,
        auth: Auth,
        root: std::path::PathBuf,
    ) -> anyhow::Result<Self> {
        let mirror = Arc::new(Mutex::new(Mirror::default()));
        let dirty = Arc::new(Mutex::new(BTreeSet::new()));
        let status = Arc::new(Mutex::new(String::new()));
        let entities = Arc::new(Mutex::new(Entities::new(0.05)));
        let chat = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(Mutex::new(None));
        let name = name.to_owned();
        let epoch = Instant::now();
        let (out, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Out>();
        let tiles = Arc::new(Mutex::new(Vec::new()));
        type Ready = (
            u32,
            Blocks,
            [f64; 3],
            Arc<WorldSky>,
            Option<std::path::PathBuf>,
            Arc<Mutex<Player>>,
        );
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<Ready>>();
        let (m, d, st, en) = (
            mirror.clone(),
            dirty.clone(),
            status.clone(),
            entities.clone(),
        );
        let (ch, cl, ti) = (chat.clone(), closed.clone(), tiles.clone());
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
                    let mut c = match connect_with(addr, &cert, &name, auth).await {
                        Ok(c) => c,
                        Err(e) => return drop(ready_tx.send(Err(e))),
                    };
                    let spawn = c.hello.spawn;
                    let cache = cache_dir(&root, c.hello.generator_version, &c.hello.fingerprint);
                    let (radius, blocks) = (c.hello.chunk_radius_m, c.blocks.clone());
                    let pl = Arc::new(Mutex::new(Player::new(
                        &c.hello,
                        Shapes::new(&c.blocks),
                        Instant::now(),
                    )));
                    let ready_with = |sky: WorldSky| -> anyhow::Result<Ready> {
                        Ok((
                            radius,
                            blocks.clone(),
                            spawn,
                            Arc::new(sky),
                            cache.clone(),
                            pl.clone(),
                        ))
                    };
                    // The sky: from the disk cache, else asked (ready once it arrives).
                    let cached = cache.as_deref().and_then(|d| {
                        let bytes = std::fs::read(sky_file(d)).ok()?;
                        WorldSky::decompress(&bytes).ok()
                    });
                    let mut ready = Some(ready_tx);
                    match cached {
                        Some(sky) => {
                            if let Some(tx) = ready.take() {
                                let _ = tx.send(ready_with(sky));
                            }
                        }
                        None => c.send(ClientMsg::Sky),
                    }
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
                            Some(msg) = out_rx.recv() => match msg {
                                // Leaving: the connection is closed, so the server sees the
                                // player go at once (not after its 30 s idle timeout).
                                Out::Msg(ClientMsg::Bye) => c.close(),
                                Out::Msg(msg) => c.send(msg),
                                Out::Commands(cmds) => c.send_commands(&cmds),
                            },
                            ev = c.events.recv() => match ev {
                                Some(Event::Column(col)) => {
                                    let (cx, cz) = (col.cx, col.cz);
                                    lock(&m).column(*col);
                                    mark(cx, cz);
                                }
                                Some(Event::Msg(ServerMsg::Blocks { chunk, lsn, changes })) => {
                                    let resends = {
                                        let mut m = lock(&m);
                                        m.blocks(chunk, lsn, changes);
                                        m.take_resends()
                                    };
                                    // An edit below a column's chunks: the mirror forgot the
                                    // column, the server sends it again.
                                    for column in resends {
                                        c.send(ClientMsg::Resend { column });
                                    }
                                    mark(chunk[0], chunk[2]);
                                }
                                Some(Event::Player(s)) => {
                                    let mut p = lock(&pl);
                                    p.set_rtt(c.rtt());
                                    p.on_state(&s, &lock(&m));
                                }
                                Some(Event::Msg(
                                    msg @ (ServerMsg::Died { .. } | ServerMsg::Respawned { .. }),
                                )) => lock(&pl).on_msg(&msg),
                                Some(Event::Entities(bytes)) => {
                                    let now = epoch.elapsed().as_secs_f64();
                                    match lock(&en).receive(&bytes, now) {
                                        Ok(seq) => c.send(ClientMsg::Ack { seqs: vec![seq] }),
                                        Err(e) => *lock(&st) = format!("entities: {e:#}"),
                                    }
                                }
                                Some(Event::Msg(ServerMsg::Forget { columns })) => {
                                    lock(&m).forget(&columns);
                                }
                                Some(Event::Msg(ServerMsg::EditRefused { pos, reason })) => {
                                    *lock(&st) = format!("edit at {pos:?} refused: {reason}");
                                }
                                Some(Event::Msg(ServerMsg::Chat(line))) => lock(&ch).push(line),
                                Some(Event::Sky(sky, bytes)) => {
                                    if let Some(d) = &cache {
                                        write_cache(&sky_file(d), &bytes);
                                    }
                                    if let Some(tx) = ready.take() {
                                        let _ = tx.send(ready_with(*sky));
                                    }
                                }
                                Some(Event::Tile(tile, bytes)) => {
                                    if let Some(d) = &cache {
                                        write_cache(&tile_file(d, tile.key), &bytes);
                                    }
                                    lock(&ti).push(*tile);
                                }
                                Some(Event::Msg(ServerMsg::Kick { reason })) | Some(Event::Closed(reason)) => {
                                    *lock(&st) = format!("disconnected: {reason}");
                                    if let Some(tx) = ready.take() {
                                        let _ = tx.send(Err(anyhow::anyhow!("{reason}")));
                                    }
                                    lock(&cl).get_or_insert(reason);
                                    break;
                                }
                                Some(Event::Msg(_)) => {}
                                None => {
                                    if let Some(tx) = ready.take() {
                                        let _ = tx.send(Err(anyhow::anyhow!("connection lost")));
                                    }
                                    lock(&cl).get_or_insert_with(|| "connection lost".into());
                                    break;
                                }
                            },
                        }
                    }
                });
            })?;
        // The sky may have to come first (about 400 KB once per world).
        let (radius_m, blocks, spawn, sky, cache, player) = ready_rx
            .recv_timeout(Duration::from_secs(60))
            .context("no answer from the server")??;
        ensure!(radius_m >= 64, "server radius {radius_m} m too small");
        Ok(Self {
            mirror,
            player,
            out,
            dirty,
            status,
            entities,
            chat,
            closed,
            spawn,
            epoch,
            radius_m,
            blocks: Arc::new(blocks),
            sky,
            tiles,
            cache,
        })
    }

    /// The block at `p` if its column is held.
    pub fn block(&self, p: BlockPos) -> Option<u16> {
        lock(&self.mirror).block(p)
    }

    /// Top of the first solid block at or below `(x, y, z)`, if its column is held and one
    /// lies within [`FLOOR_SEARCH`] blocks.
    pub fn floor_below(&self, x: f64, y: f64, z: f64) -> Option<f64> {
        let (bx, by, bz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
        let m = lock(&self.mirror);
        (by - FLOOR_SEARCH..=by)
            .rev()
            .find(|&k| {
                m.block(BlockPos::new(bx, k, bz))
                    .is_some_and(|b| self.blocks.is_solid(b))
            })
            .map(|k| f64::from(k + 1))
    }

    /// Highest solid block of the column at `(x, z)`: its height and id, if the column is
    /// held.
    pub fn surface(&self, x: f64, z: f64) -> Option<(i32, u16)> {
        let (bx, bz) = (x.floor() as i32, z.floor() as i32);
        let m = lock(&self.mirror);
        let p = BlockPos::new(bx, 0, bz).chunk();
        let col = m.get(
            p.cx.rem_euclid(CHUNKS_PER_AXIS),
            p.cz.rem_euclid(CHUNKS_PER_AXIS),
        )?;
        let top = (col.cy_min + col.chunks.len() as i32) * nascio_core::consts::CHUNK_SIZE - 1;
        let bottom = col.cy_min * nascio_core::consts::CHUNK_SIZE;
        (bottom..=top).rev().find_map(|y| {
            let b = m.block(BlockPos::new(bx, y, bz))?;
            self.blocks.is_solid(b).then_some((y, b))
        })
    }

    pub fn send(&self, m: ClientMsg) {
        let _ = self.out.send(Out::Msg(m));
    }

    /// Sends the player's commands in a datagram.
    pub fn send_commands(&self, c: PlayerCommands) {
        let _ = self.out.send(Out::Commands(c));
    }

    /// The predicted player. Taken before [`Self::lock_mirror`] when both are needed.
    pub fn lock_player(&self) -> std::sync::MutexGuard<'_, Player> {
        lock(&self.player)
    }

    pub fn lock_mirror(&self) -> std::sync::MutexGuard<'_, Mirror> {
        lock(&self.mirror)
    }

    /// Leaves the server: tells it, and gives the message a moment to go out before the
    /// process ends.
    pub fn disconnect(&self) {
        self.send(ClientMsg::Bye);
        std::thread::sleep(Duration::from_millis(250));
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

/// Columns that arrived or changed are meshed again. The player's commands go from
/// [`crate::player::walk`].
pub fn remote_sync(remote: Option<Res<Remote>>, mut streamer: ResMut<Streamer>) {
    let Some(remote) = remote else { return };
    let dirty: Vec<(i32, i32)> = std::mem::take(&mut *lock(&remote.dirty))
        .into_iter()
        .collect();
    if !dirty.is_empty() {
        streamer.request_remesh(dirty);
    }
}

/// A box standing for another player.
#[derive(Component)]
pub struct RemotePlayer(u32);

/// Back to the menu: the other players' boxes go.
pub fn clear_players(
    mut commands: Commands,
    boxes: Query<Entity, bevy::prelude::With<RemotePlayer>>,
) {
    for e in &boxes {
        commands.entity(e).despawn();
    }
}

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
