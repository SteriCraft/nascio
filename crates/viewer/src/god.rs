//! God mode (future server tool): destroy and place blocks in the 3D view. In local flight,
//! edits go to the world's [`DeltaStore`] (`world-<seed>/deltas/`, journal forced to the disk
//! at each edit), and the touched columns are meshed again; connected, they go to the server
//! and the ray reads the blocks it sent: it starts at the player's eyes and reaches 5 m (the
//! server's reach), and each edit names the newest command the view includes (`cmd`), from
//! which the server measures the reach.
//!
//! G toggles the mode. The mouse then turns the view (cursor captured) and a cross marks the
//! centre of the screen: left click destroys the block under it, right click places the
//! selected block against it, the wheel picks the block. G or Escape leaves.

use std::collections::BTreeSet;
use std::sync::Arc;

use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use nascio_core::consts::CHUNK_SIZE;
use nascio_core::coords::BlockPos;
use nascio_voxel::block::{AIR, BlockId};
use nascio_voxel::{ColumnCache, DeltaStore, Terrain};

use crate::fly::{FlyCam, RenderOrigin, SharedDeltas, SharedTerrain, Streamer};

/// Farthest block the cross reaches in local flight, m.
const REACH: f64 = 20.0;
/// Generated columns kept for block lookups.
const CACHE: usize = 64;
/// Deepest search for the floor under the camera, blocks.
const FLOOR_SEARCH: i32 = 256;

/// The block table in use: the local world's, or the server's once connected.
#[derive(Resource, Clone)]
pub struct BlockTable(pub Arc<nascio_chunk::Blocks>);

#[derive(Resource)]
pub struct God {
    pub on: bool,
    /// Block placed by the tool.
    pub block: BlockId,
    /// Block under the cross and the free cell in front of it.
    target: Option<(BlockPos, BlockPos)>,
    columns: ColumnCache,
    pub status: String,
}

impl God {
    pub fn new(block: BlockId) -> Self {
        Self {
            on: false,
            block,
            target: None,
            columns: ColumnCache::new(CACHE),
            status: String::new(),
        }
    }

    /// The block at `p`, modifications included.
    pub fn block_at(&mut self, terrain: &Terrain, deltas: &DeltaStore, p: BlockPos) -> BlockId {
        if let Some(b) = deltas.get(p) {
            return b;
        }
        let key = (p.x.div_euclid(CHUNK_SIZE), p.z.div_euclid(CHUNK_SIZE));
        let col = self.columns.get(terrain, key.0, key.1);
        col.block(
            terrain,
            p.x - key.0 * CHUNK_SIZE,
            p.y,
            p.z - key.1 * CHUNK_SIZE,
        )
    }

    /// Top of the first solid block at or below `(x, y, z)` (modifications included), if
    /// within [`FLOOR_SEARCH`] blocks.
    pub fn floor_below(
        &mut self,
        terrain: &Terrain,
        deltas: &DeltaStore,
        x: f64,
        y: f64,
        z: f64,
    ) -> Option<f64> {
        let (bx, by, bz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
        (by - FLOOR_SEARCH..=by)
            .rev()
            .find(|&k| {
                let b = self.block_at(terrain, deltas, BlockPos::new(bx, k, bz));
                terrain.blocks().is_solid(b)
            })
            .map(|k| f64::from(k + 1))
    }
}

/// First solid block along the ray (voxel walk), with the cell crossed just before it.
fn raycast(
    start: [f64; 3],
    dir: [f64; 3],
    reach: f64,
    mut solid: impl FnMut(BlockPos) -> bool,
) -> Option<(BlockPos, BlockPos)> {
    let mut cell = start.map(|v| v.floor() as i64);
    let step = dir.map(|d| if d > 0.0 { 1 } else { -1 });
    let mut t_max = [0.0; 3];
    let mut t_delta = [f64::INFINITY; 3];
    for a in 0..3 {
        if dir[a] != 0.0 {
            let next = if dir[a] > 0.0 {
                cell[a] as f64 + 1.0
            } else {
                cell[a] as f64
            };
            t_max[a] = (next - start[a]) / dir[a];
            t_delta[a] = 1.0 / dir[a].abs();
        } else {
            t_max[a] = f64::INFINITY;
        }
    }
    let pos = |c: [i64; 3]| BlockPos::new(c[0] as i32, c[1] as i32, c[2] as i32);
    let mut prev = cell;
    let mut t = 0.0;
    while t <= reach {
        if solid(pos(cell)) {
            return Some((pos(cell), pos(prev)));
        }
        prev = cell;
        let a = if t_max[0] < t_max[1] && t_max[0] < t_max[2] {
            0
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        t = t_max[a];
        t_max[a] += t_delta[a];
        cell[a] += step[a];
    }
    None
}

/// Columns whose mesh shows block `p` (its own, and the neighbours whose border holds it).
fn columns_of(p: BlockPos) -> BTreeSet<(i32, i32)> {
    let mut v = BTreeSet::new();
    for (dx, dz) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
        let q = p.offset(dx, 0, dz);
        v.insert((q.x.div_euclid(CHUNK_SIZE), q.z.div_euclid(CHUNK_SIZE)));
    }
    v
}

/// Toggle, centre ray, highlight, block choice and edits.
#[allow(clippy::too_many_arguments)]
pub fn god_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    scroll: Res<AccumulatedMouseScroll>,
    cam: Res<FlyCam>,
    origin: Res<RenderOrigin>,
    terrain: Option<Res<SharedTerrain>>,
    deltas: Option<Res<SharedDeltas>>,
    table: Option<Res<BlockTable>>,
    mut god: ResMut<God>,
    mut streamer: ResMut<Streamer>,
    mut gizmos: Gizmos,
    remote: Option<Res<crate::remote::Remote>>,
    chat: Res<crate::chat::ChatBox>,
    pause: Res<crate::pause::Pause>,
) {
    if cam.scripted || chat.open || pause.open {
        return;
    }
    let escape = god.on && keys.just_pressed(KeyCode::Escape);
    if keys.just_pressed(KeyCode::KeyG) || escape {
        god.on = !god.on && !escape;
        god.target = None;
    }
    if !god.on {
        return;
    }
    let Some(table) = table else { return };
    let blocks = &*table.0;
    // Wheel: next / previous block (air excluded).
    if scroll.delta.y != 0.0 {
        let notches = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y,
            MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
        };
        let n = blocks.len() as i32 - 1;
        let step = if notches > 0.0 { -1 } else { 1 };
        god.block = ((i32::from(god.block) - 1 + step).rem_euclid(n) + 1) as BlockId;
    }
    let p = cam.pos;
    let g = &mut *god;
    g.target = match (&remote, &terrain, &deltas) {
        // Connected: the blocks the server sent (a column not held stops nothing).
        (Some(r), _, _) => raycast(
            [p.x, p.y, p.z],
            cam.forward(),
            crate::player::EDIT_REACH_M,
            |q| r.block(q).is_some_and(|b| blocks.is_solid(b)),
        ),
        (None, Some(t), Some(d)) => {
            let store = d.snapshot();
            raycast([p.x, p.y, p.z], cam.forward(), REACH, |q| {
                let b = g.block_at(&t.0, &store, q);
                blocks.is_solid(b)
            })
        }
        _ => None,
    };
    let Some((hit, front)) = god.target else {
        return;
    };
    let corner = origin.translation(hit.x, hit.y, hit.z);
    gizmos.cube(
        Transform::from_translation(corner + Vec3::splat(0.5)).with_scale(Vec3::splat(1.02)),
        Color::srgb(1.0, 0.85, 0.2),
    );
    let edit = if buttons.just_pressed(MouseButton::Left) {
        Some((hit, AIR))
    } else if buttons.just_pressed(MouseButton::Right) {
        (front != hit).then_some((front, god.block))
    } else {
        None
    };
    let Some((q, id)) = edit else {
        return;
    };
    if let Some(remote) = remote {
        // Network mode: the server decides; its change comes back and the column is meshed
        // again then. It measures the reach from the eyes after the newest command the view
        // includes. A dead player edits nothing.
        let (dead, cmd) = {
            let p = remote.lock_player();
            (p.dead(), p.last_cmd())
        };
        if dead {
            return;
        }
        remote.send(nascio_protocol::messages::ClientMsg::SetBlock {
            pos: [q.x, q.y, q.z],
            block: id,
            cmd,
        });
        let refused = std::mem::take(&mut *remote.status.lock().unwrap_or_else(|e| e.into_inner()));
        god.status = if refused.is_empty() {
            format!(
                "sent to the server: {} at {} {} {}",
                blocks.def(id).name,
                q.x,
                q.y,
                q.z
            )
        } else {
            refused
        };
        god.target = None;
        return;
    }
    let Some(deltas) = deltas else { return };
    // Rare edits: each one is made durable at once (one `fsync`, under a millisecond).
    let result = deltas.0.set(q, id).and_then(|_| deltas.0.flush());
    god.status = match result {
        Ok(_) if id == AIR => format!("removed at {} {} {}", q.x, q.y, q.z),
        Ok(_) => format!("placed {} at {} {} {}", blocks.def(id).name, q.x, q.y, q.z),
        Err(e) => format!("save failed: {e:#}"),
    };
    god.target = None;
    streamer.request_remesh(columns_of(q));
}

/// Cross at the centre of the screen and the tool panel while the mode is on.
pub fn god_ui(
    mut contexts: EguiContexts,
    god: Res<God>,
    table: Option<Res<BlockTable>>,
    deltas: Option<Res<SharedDeltas>>,
) {
    if !god.on {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let c = ctx.content_rect().center();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("god_cross"),
    ));
    let stroke = egui::Stroke::new(2.0, egui::Color32::from_white_alpha(220));
    painter.line_segment([c - egui::vec2(9.0, 0.0), c + egui::vec2(9.0, 0.0)], stroke);
    painter.line_segment([c - egui::vec2(0.0, 9.0), c + egui::vec2(0.0, 9.0)], stroke);
    let Some(table) = table else { return };
    let blocks = &*table.0;
    let modified = deltas.map_or(String::new(), |d| {
        format!(
            "{} modified chunks (loaded regions) · ",
            d.0.loaded_chunk_count()
        )
    });
    egui::Window::new("God mode")
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -12.0])
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.label("Left click: destroy · right click: place · wheel: block · G / Esc: leave");
            ui.heading(blocks.def(god.block).name.as_str());
            ui.label(format!("{modified}{}", god.status));
        });
}
