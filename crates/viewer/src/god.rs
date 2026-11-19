//! Edits and the inventory bar. A cross marks the centre of the screen; left click destroys
//! the block under it, right click places the block of the selected slot against it, the
//! wheel moves the cursor over the [`SLOTS`] slots of the bar, and the block's name shows for
//! [`NAME_S`] above the bar when the cursor moves (author's decisions: no other message).
//!
//! Connected, the server decides: in walk mode the bar is the inventory it sends (a destroyed
//! block goes in it, a placed one comes from the slot, an empty slot places nothing); in god
//! mode (G, administrators only, confirmed by the server) the slots hold [`GOD_BLOCKS`],
//! unlimited, and the player may fly. The ray starts at the eyes and reaches 5 m (the
//! server's reach); each edit names the newest command the view includes (`cmd`).
//!
//! In local flight, G toggles god mode (Escape leaves it): edits go to the world's
//! [`DeltaStore`] (`world-<seed>/deltas/`, journal forced to the disk at each edit), and the
//! touched columns are meshed again.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use nascio_core::consts::CHUNK_SIZE;
use nascio_core::coords::BlockPos;
use nascio_protocol::inventory::SLOTS;
use nascio_protocol::messages::ClientMsg;
use nascio_voxel::block::{AIR, BlockId};
use nascio_voxel::{ColumnCache, DeltaStore, Terrain};

use crate::fly::{FlyCam, RenderOrigin, SharedDeltas, SharedTerrain, Streamer};

/// The slots in god mode, left to right (author's decision).
pub const GOD_BLOCKS: [&str; SLOTS] = [
    "dirt",
    "grass",
    "regolith",
    "gravel",
    "spruce_log",
    "spruce_leaves",
    "snow",
    "sand",
    "sandstone",
    "magma",
];
/// The block's name stays this long after the cursor moves, s.
pub const NAME_S: f64 = 5.0;
/// Farthest block the cross reaches in local flight, m.
const REACH: f64 = 20.0;
/// Generated columns kept for block lookups.
const CACHE: usize = 64;
/// Deepest search for the floor under the camera, blocks.
const FLOOR_SEARCH: i32 = 256;
/// Edge of a slot of the bar, gap between slots and margin under the bar, points.
const SLOT_PX: f32 = 46.0;
const GAP_PX: f32 = 4.0;
const MARGIN_PX: f32 = 14.0;

/// The block table in use: the local world's, or the server's once connected.
#[derive(Resource, Clone)]
pub struct BlockTable(pub Arc<nascio_chunk::Blocks>);

/// What a slot shows: a block, and how many (`None`: unlimited, god mode).
pub type Slot = Option<(BlockId, Option<u8>)>;

#[derive(Resource)]
pub struct God {
    /// God mode (connected: as the server confirmed it).
    pub on: bool,
    /// Slot under the cursor.
    pub cursor: usize,
    /// When the cursor last moved (s): the block's name shows until [`NAME_S`] later.
    moved_at: Option<f64>,
    /// Block under the cross and the free cell in front of it.
    target: Option<(BlockPos, BlockPos)>,
    columns: ColumnCache,
}

impl Default for God {
    fn default() -> Self {
        Self::new()
    }
}

impl God {
    pub fn new() -> Self {
        Self {
            on: false,
            cursor: 0,
            moved_at: None,
            target: None,
            columns: ColumnCache::new(CACHE),
        }
    }

    /// Back to the menu or to a new connection: walk mode, first slot.
    pub fn reset(&mut self) {
        self.on = false;
        self.cursor = 0;
        self.moved_at = None;
        self.target = None;
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

    /// The slots of the bar: god mode's blocks, the inventory connected, or nothing.
    pub fn slots(
        &self,
        remote: Option<&crate::remote::Remote>,
        blocks: &nascio_chunk::Blocks,
    ) -> [Slot; SLOTS] {
        if self.on {
            GOD_BLOCKS.map(|n| blocks.id(n).map(|b| (b, None)))
        } else if let Some(r) = remote {
            let inv = r.inventory.lock().unwrap_or_else(|e| e.into_inner());
            inv.slots.map(|s| s.map(|s| (s.block, Some(s.count))))
        } else {
            [None; SLOTS]
        }
    }
}

/// The bar and the cross show: connected, or in god mode in local flight.
fn active(god: &God, remote: bool) -> bool {
    remote || god.on
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

/// God mode toggle, cursor, centre ray, highlight and edits.
#[allow(clippy::too_many_arguments)]
pub fn god_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    scroll: Res<AccumulatedMouseScroll>,
    time: Res<Time<Real>>,
    cam: Res<FlyCam>,
    origin: Res<RenderOrigin>,
    terrain: Option<Res<SharedTerrain>>,
    deltas: Option<Res<SharedDeltas>>,
    table: Option<Res<BlockTable>>,
    mut god: ResMut<God>,
    mut streamer: ResMut<Streamer>,
    mut gizmos: Gizmos,
    remote: Option<Res<crate::remote::Remote>>,
    life: Res<crate::player::Life>,
    chat: Res<crate::chat::ChatBox>,
    pause: Res<crate::pause::Pause>,
) {
    // Connected, god mode is the server's.
    if let Some(r) = &remote {
        god.on = r.god.load(Ordering::Relaxed);
    }
    if cam.scripted || chat.open || pause.open {
        return;
    }
    match &remote {
        Some(r) => {
            if keys.just_pressed(KeyCode::KeyG) && r.admin && life.playing() {
                r.send(ClientMsg::God(!god.on));
            }
            if !life.playing() {
                god.target = None;
                return;
            }
        }
        None => {
            let escape = god.on && keys.just_pressed(KeyCode::Escape);
            if keys.just_pressed(KeyCode::KeyG) || escape {
                god.on = !god.on && !escape;
                god.target = None;
            }
        }
    }
    if !active(&god, remote.is_some()) {
        return;
    }
    let Some(table) = table else { return };
    let blocks = &*table.0;
    // Wheel: the cursor moves one slot (up: to the left).
    if scroll.delta.y != 0.0 {
        let notches = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y,
            MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
        };
        let step: i32 = if notches > 0.0 { -1 } else { 1 };
        god.cursor = (god.cursor as i32 + step).rem_euclid(SLOTS as i32) as usize;
        god.moved_at = Some(time.elapsed_secs_f64());
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
    } else if buttons.just_pressed(MouseButton::Right) && front != hit {
        // An empty slot places nothing.
        god.slots(remote.as_deref(), blocks)[god.cursor].map(|(b, _)| (front, b))
    } else {
        None
    };
    let Some((q, id)) = edit else {
        return;
    };
    god.target = None;
    if let Some(remote) = remote {
        // The server decides; its change (and the inventory) come back. It measures the reach
        // from the eyes after the newest command the view includes.
        let cmd = remote.lock_player().last_cmd();
        // Shown at once; undone if the server refuses it or does not answer.
        streamer.request_edit_remesh(remote.predict(q, id));
        remote.send(ClientMsg::SetBlock {
            pos: [q.x, q.y, q.z],
            block: id,
            cmd,
            slot: (id != AIR).then_some(god.cursor as u8),
        });
        return;
    }
    let Some(deltas) = deltas else { return };
    // Rare edits: each one is made durable at once (one `fsync`, under a millisecond).
    if let Err(e) = deltas.0.set(q, id).and_then(|_| deltas.0.flush()) {
        warn!("edit at {} {} {} not saved: {e:#}", q.x, q.y, q.z);
    }
    streamer.request_edit_remesh(nascio_client::columns_for_cell(q));
}

/// Icons of the blocks for the bar, made from the textures of the block table in use.
#[derive(Resource, Default)]
pub struct Icons {
    /// The table they were made for.
    table: Option<Arc<nascio_chunk::Blocks>>,
    pixels: Vec<Option<Vec<u8>>>,
    loaded: BTreeMap<BlockId, egui::TextureHandle>,
}

impl Icons {
    fn get(&mut self, ctx: &egui::Context, id: BlockId) -> Option<egui::TextureId> {
        if let Some(t) = self.loaded.get(&id) {
            return Some(t.id());
        }
        let px = self.pixels.get(usize::from(id))?.as_ref()?;
        let size = crate::textures::TEXTURE_SIZE as usize;
        let image = egui::ColorImage::from_rgba_unmultiplied([size, size], px);
        let t = ctx.load_texture(format!("block-{id}"), image, egui::TextureOptions::NEAREST);
        let tid = t.id();
        self.loaded.insert(id, t);
        Some(tid)
    }
}

/// The cross at the centre of the screen, the bar at the bottom and the name of the selected
/// block for a few seconds.
#[allow(clippy::too_many_arguments)]
pub fn god_ui(
    mut contexts: EguiContexts,
    god: Res<God>,
    table: Option<Res<BlockTable>>,
    remote: Option<Res<crate::remote::Remote>>,
    life: Res<crate::player::Life>,
    dir: Res<crate::textures::TextureDir>,
    time: Res<Time<Real>>,
    mut icons: ResMut<Icons>,
) {
    if !active(&god, remote.is_some()) || !life.playing() {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    // Under the windows (pause menu, settings, chat…).
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("god_cross"),
    ));
    let screen = ctx.content_rect();
    let c = screen.center();
    let stroke = egui::Stroke::new(2.0, egui::Color32::from_white_alpha(220));
    painter.line_segment([c - egui::vec2(8.0, 0.0), c + egui::vec2(8.0, 0.0)], stroke);
    painter.line_segment([c - egui::vec2(0.0, 8.0), c + egui::vec2(0.0, 8.0)], stroke);
    let Some(table) = table else { return };
    let blocks = &table.0;
    if !icons.table.as_ref().is_some_and(|t| Arc::ptr_eq(t, blocks)) {
        *icons = Icons {
            table: Some(blocks.clone()),
            pixels: crate::textures::icons(&dir.0, blocks),
            loaded: BTreeMap::new(),
        };
    }
    let slots = god.slots(remote.as_deref(), blocks);
    let width = SLOTS as f32 * SLOT_PX + (SLOTS - 1) as f32 * GAP_PX;
    let left = c.x - width / 2.0;
    let top = screen.bottom() - MARGIN_PX - SLOT_PX;
    for (i, slot) in slots.iter().enumerate() {
        let x = left + i as f32 * (SLOT_PX + GAP_PX);
        let r = egui::Rect::from_min_size(egui::pos2(x, top), egui::vec2(SLOT_PX, SLOT_PX));
        painter.rect_filled(r, 4.0, egui::Color32::from_black_alpha(140));
        if let Some((b, count)) = *slot {
            let inner = r.shrink(5.0);
            match icons.get(ctx, b) {
                Some(tex) => {
                    painter.image(
                        tex,
                        inner,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                None => {
                    let [r8, g8, b8] = blocks.def(b).color;
                    painter.rect_filled(inner, 2.0, egui::Color32::from_rgb(r8, g8, b8));
                }
            }
            if let Some(n) = count {
                let at = r.right_bottom() - egui::vec2(4.0, 2.0);
                let font = egui::FontId::proportional(14.0);
                let text = n.to_string();
                painter.text(
                    at + egui::vec2(1.0, 1.0),
                    egui::Align2::RIGHT_BOTTOM,
                    &text,
                    font.clone(),
                    egui::Color32::BLACK,
                );
                painter.text(
                    at,
                    egui::Align2::RIGHT_BOTTOM,
                    &text,
                    font,
                    egui::Color32::WHITE,
                );
            }
        }
        let (w, colour) = if i == god.cursor {
            (3.0, egui::Color32::WHITE)
        } else {
            (1.0, egui::Color32::from_white_alpha(70))
        };
        painter.rect_stroke(
            r,
            4.0,
            egui::Stroke::new(w, colour),
            egui::StrokeKind::Inside,
        );
    }
    // The name of the selected block, a few seconds after the cursor moved.
    let recent = god
        .moved_at
        .is_some_and(|t| time.elapsed_secs_f64() - t < NAME_S);
    if let (true, Some((b, _))) = (recent, slots[god.cursor]) {
        let at = egui::pos2(c.x, top - 10.0);
        let font = egui::FontId::proportional(18.0);
        let name = blocks.def(b).name.as_str();
        painter.text(
            at + egui::vec2(1.0, 1.0),
            egui::Align2::CENTER_BOTTOM,
            name,
            font.clone(),
            egui::Color32::BLACK,
        );
        painter.text(
            at,
            egui::Align2::CENTER_BOTTOM,
            name,
            font,
            egui::Color32::WHITE,
        );
    }
}
