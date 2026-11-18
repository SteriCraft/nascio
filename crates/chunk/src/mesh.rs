//! Greedy meshing of a padded chunk, with per-vertex ambient occlusion.
//!
//! Engine-agnostic output: plain vertex arrays, positions relative to the chunk origin.
//! Faces are merged when they share block and corner occlusion (the classic "greedy" pass), and
//! quads are split along the diagonal that keeps occlusion gradients isotropic.
//!
//! Faces may carry a block texture (a layer of a texture array, see `Blocks::face_texture`):
//! the layer goes in the material channel and the texture coordinates, in blocks, in `uv`, so
//! that a repeating sampler tiles the image over merged quads. Without textures, faces keep
//! the block colours.

use crate::block::{AIR, BlockId, Blocks, FaceTint, Matter};
use crate::chunk::{N, NP, PaddedChunk};
use crate::tint::ChunkTint;

/// Brightness factor per occlusion level (0 = fully occluded corner, 3 = open).
const AO_CURVE: [f32; 4] = [0.45, 0.65, 0.82, 1.0];
/// Opacity of the water surface over one block of water, and its limit over deep water
/// (below 0.99: the shader recognises water by its alpha).
pub const WATER_ALPHA: f32 = 0.4;
pub const WATER_ALPHA_DEEP: f32 = 0.97;
/// Depth scale of the water opacity, blocks.
const WATER_DEPTH_SCALE: f32 = 5.0;

/// Opacity of water `depth` blocks deep: light over shallow rivers, nearly opaque over the sea.
pub fn water_alpha(depth: u8) -> f32 {
    let d = f32::from(depth.max(1)) - 1.0;
    (1.0 - (1.0 - WATER_ALPHA) * (-d / WATER_DEPTH_SCALE).exp()).min(WATER_ALPHA_DEEP)
}

/// Albedo factor of chunk water of opacity `alpha`: from 0.95 over light rivers to 0.5 over a
/// nearly opaque sea whose floor may not be loaded, towards the deep-water tone of the horizon
/// (applied to the vertex colour of water quads).
pub fn water_shade(alpha: f32) -> f32 {
    let deep = ((alpha - WATER_ALPHA) / (WATER_ALPHA_DEEP - WATER_ALPHA)).clamp(0.0, 1.0);
    0.95 + (0.5 - 0.95) * deep
}

/// Kinds of surface in the material channel (`x` of [`MeshData::material`]); `curvature.wesl`
/// has the same constants (checked by a test in the viewer). Vertex alphas are true opacities.
pub const MATERIAL_GROUND: f32 = 0.0;
/// Water: sun glint, no snow.
pub const MATERIAL_WATER: f32 = 1.0;
/// Foliage: opaque, its sides keep their green under the seasonal snow.
pub const MATERIAL_FOLIAGE: f32 = 2.0;
/// `y` of [`MeshData::material`] on a face without texture.
pub const NO_TEXTURE: f32 = -1.0;

/// Vertex arrays of one mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Linear RGBA.
    pub colors: Vec<[f32; 4]>,
    /// Material channel (UV0 for the renderer): `x` the kind ([`MATERIAL_WATER`]…), `y` the
    /// layer of the face texture in the block texture array as `f32`, or [`NO_TEXTURE`] (audit
    /// R3, F4: no tags in the alpha).
    pub material: Vec<[f32; 2]>,
    /// UV_1 for the renderer: texture coordinates in blocks (the texture repeats once per
    /// block), local to the chunk.
    pub uv: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn quad_count(&self) -> usize {
        self.indices.len() / 6
    }

    fn push_quad(
        &mut self,
        corners: [[f32; 3]; 4],
        normal: [f32; 3],
        colors: [[f32; 4]; 4],
        material: [f32; 2],
        uv: [[f32; 2]; 4],
        flip: bool,
    ) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&corners);
        self.normals.extend_from_slice(&[normal; 4]);
        self.colors.extend_from_slice(&colors);
        self.material.extend_from_slice(&[material; 4]);
        self.uv.extend_from_slice(&uv);
        if flip {
            self.indices.extend_from_slice(&[
                base + 1,
                base + 2,
                base + 3,
                base + 1,
                base + 3,
                base,
            ]);
        } else {
            self.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
}

/// Meshes of one chunk: opaque terrain and the water surface (drawn translucent).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChunkMesh {
    pub opaque: MeshData,
    pub water: MeshData,
}

impl ChunkMesh {
    pub fn is_empty(&self) -> bool {
        self.opaque.is_empty() && self.water.is_empty()
    }
}

/// Face drawn on `block` when its neighbour across the face is `neighbour` (`Some(water)`):
/// the per-cell rule, applied on whole rows by [`mesh_chunk`] (kept for the reference mesher).
fn face(blocks: &Blocks, block: BlockId, neighbour: BlockId) -> Option<bool> {
    match (blocks.matter(block), blocks.matter(neighbour)) {
        (Matter::Solid, Matter::Empty | Matter::Fluid | Matter::Layer) => Some(false),
        (Matter::Layer, Matter::Empty | Matter::Fluid) => Some(false),
        (Matter::Fluid, Matter::Empty) => Some(true),
        _ => None,
    }
}

/// Mask entry: block and corner occlusion of one unit face.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Face {
    block: BlockId,
    water: bool,
    /// Water depth below the face's column (blocks), 0 off water.
    depth: u8,
    /// Thin layer (snow cover): drawn `LAYER_H` thick at the bottom of its cell.
    layer: bool,
    /// Foliage: opaque, tagged by [`FOLIAGE_ALPHA`].
    foliage: bool,
    ao: [u8; 4],
}

/// Thickness of a [`Matter::Layer`] block, m.
pub const LAYER_H: f32 = 0.125;

/// Rows of a padded chunk, one per `(y, z)` in `-1..=32`, index `(z + 1) + NP · (y + 1)`.
const ROWS: usize = NP * NP;

/// Reusable buffers of [`mesh_chunk`] (one set per thread).
struct Scratch {
    /// Bit `x + 1` set where the block at `(x, y, z)` has that matter (padded rows).
    solid: Vec<u64>,
    layer: Vec<u64>,
    fluid: Vec<u64>,
    empty: Vec<u64>,
    /// Visible faces per direction `2 · d + (sign < 0)`: bit `x` of word `z + N · y`.
    faces: [Vec<u32>; 6],
    mask: Vec<Option<Face>>,
}

impl Scratch {
    fn new() -> Self {
        Self {
            solid: vec![0; ROWS],
            layer: vec![0; ROWS],
            fluid: vec![0; ROWS],
            empty: vec![0; ROWS],
            faces: std::array::from_fn(|_| vec![0; N * N]),
            mask: vec![None; N * N],
        }
    }
}

thread_local! {
    static SCRATCH: std::cell::RefCell<Scratch> = std::cell::RefCell::new(Scratch::new());
}

/// Greedy mesh of the inner 32³ of a padded chunk, grass tinted by `tint`.
///
/// The visible faces are found with bit masks (one `u64` per padded row and matter), so the
/// cost follows the number of faces rather than the volume (audit M5). Same output as the
/// per-cell rule (solid against empty, fluid or layer; layer against empty or fluid; fluid
/// against empty; checked by the tests).
pub fn mesh_chunk(p: &PaddedChunk, blocks: &Blocks, tint: &ChunkTint) -> ChunkMesh {
    let mut out = ChunkMesh::default();
    let first = p.blocks[0];
    if p.blocks.iter().all(|&b| b == first) {
        // Uniform (air, rock or water): no face anywhere.
        return out;
    }
    SCRATCH.with(|s| mesh_into(&mut out, &mut s.borrow_mut(), p, blocks, tint));
    out
}

fn mesh_into(
    out: &mut ChunkMesh,
    s: &mut Scratch,
    p: &PaddedChunk,
    blocks: &Blocks,
    tint: &ChunkTint,
) {
    // Matter masks of the padded rows.
    for (r, row) in p.blocks.as_chunks::<NP>().0.iter().enumerate() {
        let (mut so, mut la, mut fl, mut em) = (0u64, 0u64, 0u64, 0u64);
        for (x, &b) in row.iter().enumerate() {
            let bit = 1u64 << x;
            match blocks.matter(b) {
                Matter::Solid => so |= bit,
                Matter::Layer => la |= bit,
                Matter::Fluid => fl |= bit,
                Matter::Empty => em |= bit,
            }
        }
        (s.solid[r], s.layer[r], s.fluid[r], s.empty[r]) = (so, la, fl, em);
    }
    // Visible faces per direction (the rule of `face`, on whole rows).
    let mut any_face = false;
    for (k, faces) in s.faces.iter_mut().enumerate() {
        let (d, sign) = (k / 2, if k % 2 == 0 { 1i32 } else { -1 });
        let step = match d {
            1 => sign * NP as i32,
            2 => sign,
            _ => 0,
        };
        for y in 0..N {
            for z in 0..N {
                let r = (z + 1) + NP * (y + 1);
                let nr = (r as i32 + step) as usize;
                let nb = |m: &[u64]| match (d, sign) {
                    (0, 1) => m[nr] >> 1,
                    (0, _) => m[nr] << 1,
                    _ => m[nr],
                };
                let (ne, nf, nl) = (nb(&s.empty), nb(&s.fluid), nb(&s.layer));
                let vis =
                    (s.solid[r] & (ne | nf | nl)) | (s.layer[r] & (ne | nf)) | (s.fluid[r] & ne);
                let w = (vis >> 1) as u32;
                any_face |= w != 0;
                faces[z + N * y] = w;
            }
        }
    }
    if !any_face {
        return;
    }
    let n = N as i32;
    let mask = &mut s.mask;
    mask.fill(None);
    for d in 0..3usize {
        let (u, v) = ((d + 1) % 3, (d + 2) % 3);
        for sign in [1i32, -1] {
            let faces = &s.faces[2 * d + usize::from(sign < 0)];
            let mut normal = [0.0f32; 3];
            normal[d] = sign as f32;
            // Slices holding at least one face.
            let mut slices = 0u32;
            for (j, &w) in faces.iter().enumerate() {
                slices |= match d {
                    0 => w,
                    1 => u32::from(w != 0) << (j / N),
                    _ => u32::from(w != 0) << (j % N),
                };
            }
            while slices != 0 {
                let i = slices.trailing_zeros() as i32;
                slices &= slices - 1;
                // Mask of visible faces on slice `i` looking along `sign · e_d`.
                for b in 0..n {
                    for a in 0..n {
                        let mut c = [0i32; 3];
                        c[d] = i;
                        c[u] = a;
                        c[v] = b;
                        if faces[(c[2] + n * c[1]) as usize] >> c[0] & 1 == 0 {
                            continue;
                        }
                        let block = p.get(c[0], c[1], c[2]);
                        let mut nb = c;
                        nb[d] += sign;
                        let matter = blocks.matter(block);
                        let water = matter == Matter::Fluid;
                        let ao = if water {
                            [3; 4]
                        } else {
                            corner_ao(p, blocks, nb, u, v)
                        };
                        mask[a as usize + N * b as usize] = Some(Face {
                            block,
                            water,
                            depth: if water {
                                tint.water_depth[(c[0] + n * c[2]) as usize]
                            } else {
                                0
                            },
                            layer: matter == Matter::Layer,
                            foliage: blocks.is_foliage(block),
                            ao,
                        });
                    }
                }
                // Greedy merge (consumes every face: the mask is empty afterwards).
                let plane = (i + i32::from(sign > 0)) as f32;
                for b in 0..N {
                    let mut a = 0;
                    while a < N {
                        let Some(f) = mask[a + N * b] else {
                            a += 1;
                            continue;
                        };
                        let mut w = 1;
                        while a + w < N && mask[a + w + N * b] == Some(f) {
                            w += 1;
                        }
                        let mut h = 1;
                        'grow: while b + h < N {
                            for k in 0..w {
                                if mask[a + k + N * (b + h)] != Some(f) {
                                    break 'grow;
                                }
                            }
                            h += 1;
                        }
                        for bb in b..b + h {
                            for aa in a..a + w {
                                mask[aa + N * bb] = None;
                            }
                        }
                        emit(
                            out,
                            blocks,
                            tint,
                            f,
                            d,
                            u,
                            v,
                            sign,
                            plane,
                            normal,
                            [a, b, w, h],
                        );
                        a += w;
                    }
                }
            }
        }
    }
}

/// Occlusion of the four corners of a face whose air-side cell is `nb`, in quad corner order
/// `(-u,-v), (+u,-v), (+u,+v), (-u,+v)`.
fn corner_ao(p: &PaddedChunk, blocks: &Blocks, nb: [i32; 3], u: usize, v: usize) -> [u8; 4] {
    let solid = |du: i32, dv: i32| {
        let mut c = nb;
        c[u] += du;
        c[v] += dv;
        u8::from(blocks.is_solid(p.get(c[0], c[1], c[2])))
    };
    [(-1, -1), (1, -1), (1, 1), (-1, 1)].map(|(du, dv)| {
        let (s1, s2) = (solid(du, 0), solid(0, dv));
        if s1 == 1 && s2 == 1 {
            0
        } else {
            3 - (s1 + s2 + solid(du, dv))
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn emit(
    out: &mut ChunkMesh,
    blocks: &Blocks,
    tint: &ChunkTint,
    f: Face,
    d: usize,
    u: usize,
    v: usize,
    sign: i32,
    plane: f32,
    normal: [f32; 3],
    [a, b, w, h]: [usize; 4],
) {
    let corner = |ca: usize, cb: usize| {
        let mut p = [0.0f32; 3];
        p[d] = plane;
        p[u] = ca as f32;
        p[v] = cb as f32;
        p
    };
    let mut corners = [
        corner(a, b),
        corner(a + w, b),
        corner(a + w, b + h),
        corner(a, b + h),
    ];
    if f.layer {
        // The top of a layer is `LAYER_H` above its cell's floor; layers never stack, so a
        // side quad is one block high.
        let floor = corners.iter().map(|c| c[1]).fold(f32::INFINITY, f32::min);
        for c in &mut corners {
            if c[1] > floor || (d == 1 && sign > 0) {
                c[1] += LAYER_H - 1.0;
            }
        }
    }
    let alpha = if f.water { water_alpha(f.depth) } else { 1.0 };
    let texture = blocks.face_texture(f.block, d, sign);
    let kind = if f.water {
        MATERIAL_WATER
    } else if f.foliage {
        MATERIAL_FOLIAGE
    } else {
        MATERIAL_GROUND
    };
    let material = [kind, texture.map_or(NO_TEXTURE, |t| f32::from(t.layer))];
    let shade = if f.water { water_shade(alpha) } else { 1.0 };
    let tinted = tint.grass == Some(f.block);
    let block_rgb = blocks.linear(f.block);
    let mut colors = [0usize, 1, 2, 3].map(|k| {
        let (x, z) = (corners[k][0], corners[k][2]);
        let rgb = match texture.map(|t| t.tint) {
            Some(FaceTint::None) => [1.0; 3],
            Some(FaceTint::Climate) if tinted => tint.at(x, z),
            Some(FaceTint::Climate) => [1.0; 3],
            Some(FaceTint::Fixed(c)) => c,
            None if tinted => tint.at(x, z),
            None => block_rgb,
        };
        let a = AO_CURVE[usize::from(f.ao[k])] * shade;
        [rgb[0] * a, rgb[1] * a, rgb[2] * a, alpha]
    });
    // Anisotropy fix: split along the diagonal joining the brighter pair.
    let flip = u16::from(f.ao[0]) + u16::from(f.ao[2]) < u16::from(f.ao[1]) + u16::from(f.ao[3]);
    if sign < 0 {
        // Reverse winding so the face looks along −e_d (counter-clockwise front faces).
        corners.swap(1, 3);
        colors.swap(1, 3);
        // Swapping corners 1 and 3 keeps the 0–2 diagonal, so `flip` is unchanged.
    }
    // Texture coordinates from the final corners: the top edge of a block is the top of the
    // image; `right` is the horizontal direction to the right of a viewer facing the face.
    let uv = corners.map(|c| {
        if d == 1 {
            [c[0], c[2]]
        } else {
            let u = match (d, sign > 0) {
                (0, true) => -c[2],
                (0, false) => c[2],
                (_, true) => c[0],
                (_, false) => -c[0],
            };
            [u, -c[1]]
        }
    });
    let target = if f.water {
        &mut out.water
    } else {
        &mut out.opaque
    };
    target.push_quad(corners, normal, colors, material, uv, flip);
}

/// The per-cell definition (the pre-M7 implementation), reference of the bit-mask mesher in
/// tests (here and in `nascio-voxel` on generated terrain).
#[doc(hidden)]
pub fn mesh_reference(p: &PaddedChunk, blocks: &Blocks, tint: &ChunkTint) -> ChunkMesh {
    let mut out = ChunkMesh::default();
    if p.is_uniform(AIR) || p.blocks.iter().all(|&b| blocks.is_solid(b)) {
        return out;
    }
    let n = N as i32;
    let mut mask: Vec<Option<Face>> = vec![None; N * N];
    for d in 0..3usize {
        let (u, v) = ((d + 1) % 3, (d + 2) % 3);
        for sign in [1i32, -1] {
            let mut normal = [0.0f32; 3];
            normal[d] = sign as f32;
            for i in 0..n {
                // Build the mask of visible faces on slice `i` looking along `sign · e_d`.
                for b in 0..n {
                    for a in 0..n {
                        let mut c = [0i32; 3];
                        c[d] = i;
                        c[u] = a;
                        c[v] = b;
                        let block = p.get(c[0], c[1], c[2]);
                        let mut nb = c;
                        nb[d] += sign;
                        let neighbour = p.get(nb[0], nb[1], nb[2]);
                        mask[a as usize + N * b as usize] =
                            face(blocks, block, neighbour).map(|water| {
                                let ao = if water {
                                    [3; 4]
                                } else {
                                    corner_ao(p, blocks, nb, u, v)
                                };
                                Face {
                                    block,
                                    water,
                                    depth: if water {
                                        tint.water_depth[(c[0] + n * c[2]) as usize]
                                    } else {
                                        0
                                    },
                                    layer: blocks.matter(block) == Matter::Layer,
                                    foliage: blocks.is_foliage(block),
                                    ao,
                                }
                            });
                    }
                }
                // Greedy merge.
                let plane = (i + i32::from(sign > 0)) as f32;
                for b in 0..N {
                    let mut a = 0;
                    while a < N {
                        let Some(f) = mask[a + N * b] else {
                            a += 1;
                            continue;
                        };
                        let mut w = 1;
                        while a + w < N && mask[a + w + N * b] == Some(f) {
                            w += 1;
                        }
                        let mut h = 1;
                        'grow: while b + h < N {
                            for k in 0..w {
                                if mask[a + k + N * (b + h)] != Some(f) {
                                    break 'grow;
                                }
                            }
                            h += 1;
                        }
                        for bb in b..b + h {
                            for aa in a..a + w {
                                mask[aa + N * bb] = None;
                            }
                        }
                        emit(
                            &mut out,
                            blocks,
                            tint,
                            f,
                            d,
                            u,
                            v,
                            sign,
                            plane,
                            normal,
                            [a, b, w, h],
                        );
                        a += w;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ids() -> (Blocks, BlockId, BlockId) {
        let b = Blocks::builtin();
        let (stone, water) = (b.id("granite").unwrap(), b.id("water").unwrap());
        (b, stone, water)
    }

    fn padded(f: impl Fn(i32, i32, i32) -> BlockId) -> PaddedChunk {
        let mut blocks = Box::new([AIR; NP * NP * NP]);
        for y in -1..=32 {
            for z in -1..=32 {
                for x in -1..=32 {
                    blocks[PaddedChunk::index(x, y, z)] = f(x, y, z);
                }
            }
        }
        PaddedChunk { blocks }
    }

    #[test]
    fn flat_ground_is_one_quad() {
        let (b, stone, _) = ids();
        let p = padded(|_, y, _| if y < 10 { stone } else { AIR });
        let m = mesh_chunk(&p, &b, &ChunkTint::NONE);
        assert_eq!(m.opaque.quad_count(), 1);
        assert!(m.water.is_empty());
        assert_eq!(m.opaque.normals[0], [0.0, 1.0, 0.0]);
        assert!(m.opaque.positions.iter().all(|p| p[1] == 10.0));
    }

    #[test]
    fn single_block_has_six_faces_facing_out() {
        let (b, stone, _) = ids();
        let p = padded(|x, y, z| if (x, y, z) == (4, 5, 6) { stone } else { AIR });
        let m = mesh_chunk(&p, &b, &ChunkTint::NONE);
        assert_eq!(m.opaque.quad_count(), 6);
        // Each triangle's winding must agree with its normal (counter-clockwise front).
        for t in m.opaque.indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.opaque.positions[i as usize]);
            let n = m.opaque.normals[t[0] as usize];
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let cross = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            assert!(cross[0] * n[0] + cross[1] * n[1] + cross[2] * n[2] > 0.0);
        }
    }

    #[test]
    fn water_surface_only_against_air() {
        let (b, stone, water) = ids();
        let p = padded(|_, y, _| match y {
            ..3 => stone,
            3..20 => water,
            _ => AIR,
        });
        let m = mesh_chunk(&p, &b, &ChunkTint::NONE);
        assert_eq!(m.water.quad_count(), 1);
        assert_eq!(m.opaque.quad_count(), 1);
        assert!(m.water.positions.iter().all(|p| p[1] == 20.0));
    }

    #[test]
    fn buried_and_empty_chunks_have_no_mesh() {
        let (b, stone, _) = ids();
        assert!(mesh_chunk(&padded(|_, _, _| stone), &b, &ChunkTint::NONE).is_empty());
        assert!(mesh_chunk(&padded(|_, _, _| AIR), &b, &ChunkTint::NONE).is_empty());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        /// Random mixes of every matter (rock, snow layer, leaves, water, air), with ragged
        /// borders: the bit-mask mesher gives exactly the per-cell definition.
        #[test]
        fn same_mesh_as_the_reference_on_random_chunks(
            seed in any::<u64>(),
            density in 1u64..16,
        ) {
            let mut b = Blocks::builtin();
            let (grass, water, snow) = (
                b.id("grass").unwrap(),
                b.id("water").unwrap(),
                b.id("snow_layer").unwrap(),
            );
            let mut t = vec![BlockTextures::default(); b.len()];
            t[usize::from(grass)] = BlockTextures {
                top: tex(1, FaceTint::Climate),
                side: None,
                bottom: tex(2, FaceTint::None),
            };
            t[usize::from(water)] = BlockTextures {
                top: tex(3, FaceTint::Fixed([0.3, 0.5, 0.9])),
                side: tex(3, FaceTint::None),
                bottom: None,
            };
            t[usize::from(snow)] = BlockTextures {
                top: None,
                side: tex(4, FaceTint::None),
                bottom: None,
            };
            b.set_textures(t).unwrap();
            let id = |name: &str| b.id(name).expect(name);
            let palette = [
                AIR,
                id("granite"),
                id("water"),
                id("grass"),
                id("snow_layer"),
                id("spruce_leaves"),
            ];
            let p = padded(|x, y, z| {
                let mut h = seed ^ (((x + 1) + 34 * ((z + 1) + 34 * (y + 1))) as u64)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15);
                h ^= h >> 29;
                h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                h ^= h >> 32;
                // Mostly air above a ground, so that faces merge as well as break up.
                if h % 16 >= density && y > 8 {
                    AIR
                } else {
                    palette[(h >> 8) as usize % palette.len()]
                }
            });
            let mut tint = ChunkTint::NONE;
            tint.grass = Some(id("grass"));
            for (k, d) in tint.water_depth.iter_mut().enumerate() {
                *d = (seed as usize + k) as u8 % 7;
            }
            prop_assert_eq!(mesh_chunk(&p, &b, &tint), mesh_reference(&p, &b, &tint));
        }
    }

    use crate::block::{BlockTextures, FaceTexture};

    fn tex(layer: u16, tint: FaceTint) -> Option<FaceTexture> {
        Some(FaceTexture { layer, tint })
    }

    fn with_textures(b: &mut Blocks, set: &[(BlockId, BlockTextures)]) {
        let mut t = vec![BlockTextures::default(); b.len()];
        for &(id, bt) in set {
            t[usize::from(id)] = bt;
        }
        b.set_textures(t).unwrap();
    }

    #[test]
    fn without_textures_the_material_has_no_layer_and_water_is_shaded_in_the_colour() {
        let (b, stone, water) = ids();
        let p = padded(|_, y, _| match y {
            ..3 => stone,
            3..20 => water,
            _ => AIR,
        });
        let mut tint = ChunkTint::NONE;
        tint.water_depth.fill(4);
        let m = mesh_chunk(&p, &b, &tint);
        for d in [&m.opaque, &m.water] {
            assert_eq!(d.uv.len(), d.positions.len());
            assert!(d.material.iter().all(|m| m[1] == NO_TEXTURE));
        }
        let alpha = water_alpha(4);
        let w = b.linear(water);
        let k = water_shade(alpha);
        assert!(!m.water.colors.is_empty());
        assert!(
            m.water
                .colors
                .iter()
                .all(|c| *c == [w[0] * k, w[1] * k, w[2] * k, alpha])
        );
    }

    #[test]
    fn textured_flat_ground_is_one_quad_with_layer_and_uvs() {
        let (mut b, stone, _) = ids();
        let bt = BlockTextures {
            top: tex(3, FaceTint::None),
            ..Default::default()
        };
        with_textures(&mut b, &[(stone, bt)]);
        let p = padded(|_, y, _| if y < 10 { stone } else { AIR });
        let m = mesh_chunk(&p, &b, &ChunkTint::NONE).opaque;
        assert_eq!(m.quad_count(), 1);
        assert!(m.material.iter().all(|m| *m == [MATERIAL_GROUND, 3.0]));
        let mut uvs = m.uv.clone();
        uvs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(uvs, [[0.0, 0.0], [0.0, 32.0], [32.0, 0.0], [32.0, 32.0]]);
        for i in 0..m.positions.len() {
            assert_eq!(m.uv[i], [m.positions[i][0], m.positions[i][2]]);
        }
        assert!(m.colors.iter().all(|c| *c == [1.0, 1.0, 1.0, 1.0]));
    }

    #[test]
    fn side_uvs_are_upright_and_face_right() {
        let (mut b, stone, _) = ids();
        let bt = BlockTextures {
            top: tex(1, FaceTint::None),
            side: tex(2, FaceTint::None),
            bottom: tex(3, FaceTint::None),
        };
        with_textures(&mut b, &[(stone, bt)]);
        let p = padded(|x, y, z| if (x, y, z) == (4, 5, 6) { stone } else { AIR });
        let m = mesh_chunk(&p, &b, &ChunkTint::NONE).opaque;
        assert_eq!(m.quad_count(), 6);
        for q in 0..6 {
            let vs = 4 * q..4 * q + 4;
            let n = m.normals[4 * q];
            let layer = m.material[4 * q][1];
            if n[1] > 0.0 {
                assert_eq!(layer, 1.0);
            } else if n[1] < 0.0 {
                assert_eq!(layer, 3.0);
            } else {
                assert_eq!(layer, 2.0);
                // Right of a viewer facing the face: +x -> -z, -x -> +z, +z -> +x, -z -> -x.
                let right = [n[2], 0.0, -n[0]];
                let dotr = |p: [f32; 3]| p[0] * right[0] + p[2] * right[2];
                for i in vs.clone() {
                    assert_eq!(m.uv[i][1], -m.positions[i][1]);
                    for j in vs.clone() {
                        let (pi, pj) = (m.positions[i], m.positions[j]);
                        assert_eq!(m.uv[j][0] - m.uv[i][0], dotr(pj) - dotr(pi));
                    }
                }
            }
        }
    }

    #[test]
    fn face_tints() {
        let b0 = Blocks::builtin();
        let (grass, stone) = (b0.id("grass").unwrap(), b0.id("granite").unwrap());
        let mut tint = ChunkTint::NONE;
        tint.grass = Some(grass);
        tint.corners = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.5, 0.5, 0.0],
        ];
        let single = |b: &Blocks, id| {
            let p = padded(|x, y, z| if (x, y, z) == (4, 5, 6) { id } else { AIR });
            mesh_chunk(&p, b, &tint).opaque
        };
        let top_vertices = |m: &MeshData| {
            (0..m.positions.len())
                .filter(|&i| m.normals[i] == [0.0, 1.0, 0.0])
                .collect::<Vec<_>>()
        };
        // Climate on the tinted block.
        let mut b = b0.clone();
        let bt = BlockTextures {
            top: tex(1, FaceTint::Climate),
            ..Default::default()
        };
        with_textures(&mut b, &[(grass, bt), (stone, bt)]);
        let m = single(&b, grass);
        let tops = top_vertices(&m);
        assert_eq!(tops.len(), 4);
        for i in tops {
            let at = tint.at(m.positions[i][0], m.positions[i][2]);
            let ao = AO_CURVE[3];
            assert_eq!(m.colors[i], [at[0] * ao, at[1] * ao, at[2] * ao, 1.0]);
        }
        // Climate on a block that is not the tinted one: white.
        let m = single(&b, stone);
        for i in top_vertices(&m) {
            assert_eq!(m.colors[i], [1.0, 1.0, 1.0, 1.0]);
        }
        // Fixed colour.
        let c = [0.2, 0.4, 0.6];
        let bt = BlockTextures {
            top: tex(1, FaceTint::Fixed(c)),
            ..Default::default()
        };
        with_textures(&mut b, &[(grass, bt)]);
        let m = single(&b, grass);
        for i in top_vertices(&m) {
            assert_eq!(m.colors[i], [c[0], c[1], c[2], 1.0]);
        }
    }

    #[test]
    fn snow_layer_side_shows_the_bottom_eighth_of_the_texture() {
        let mut b = Blocks::builtin();
        let (stone, snow) = (b.id("granite").unwrap(), b.id("snow_layer").unwrap());
        let bt = BlockTextures {
            side: tex(5, FaceTint::None),
            ..Default::default()
        };
        with_textures(&mut b, &[(snow, bt)]);
        let p = padded(|x, y, z| match (x, y, z) {
            (4, 4, 6) => stone,
            (4, 5, 6) => snow,
            _ => AIR,
        });
        let m = mesh_chunk(&p, &b, &ChunkTint::NONE).opaque;
        let mut sides = 0;
        for q in 0..m.quad_count() {
            if m.material[4 * q][1] != 5.0 {
                continue;
            }
            sides += 1;
            let v = (4 * q..4 * q + 4).map(|i| m.uv[i][1]);
            let lo = v.clone().fold(f32::MAX, f32::min);
            let hi = v.fold(f32::MIN, f32::max);
            assert_eq!(hi - lo, LAYER_H);
        }
        assert_eq!(sides, 4);
    }

    #[test]
    fn uniform_water_has_no_mesh() {
        let (b, _, water) = ids();
        assert!(mesh_chunk(&padded(|_, _, _| water), &b, &ChunkTint::NONE).is_empty());
    }
}
