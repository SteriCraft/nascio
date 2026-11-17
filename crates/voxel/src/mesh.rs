//! Greedy meshing of a padded chunk, with per-vertex ambient occlusion.
//!
//! Engine-agnostic output: plain vertex arrays, positions relative to the chunk origin.
//! Faces are merged when they share block and corner occlusion (the classic "greedy" pass), and
//! quads are split along the diagonal that keeps occlusion gradients isotropic.

// Rendering only (colours, previews): platform maths is fine here.
#![allow(clippy::disallowed_methods)]

use crate::block::{BlockId, Blocks, Matter};
use crate::chunk::{N, NP, PaddedChunk};
use crate::terrain::ChunkTint;

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

/// Vertex arrays of one mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Linear RGBA.
    pub colors: Vec<[f32; 4]>,
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
        flip: bool,
    ) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&corners);
        self.normals.extend_from_slice(&[normal; 4]);
        self.colors.extend_from_slice(&colors);
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
#[cfg(test)]
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

/// Vertex alpha of foliage faces: opaque material, but the shader keeps their sides green
/// under the seasonal snow (water is below 0.99, other faces 1).
pub const FOLIAGE_ALPHA: f32 = 0.995;

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
                            blocks.linear(f.block),
                            (tint.grass == Some(f.block)).then_some(tint),
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
    rgb: [f32; 3],
    tint: Option<&ChunkTint>,
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
    let alpha = if f.water {
        water_alpha(f.depth)
    } else if f.foliage {
        FOLIAGE_ALPHA
    } else {
        1.0
    };
    let mut colors = [0usize, 1, 2, 3].map(|k| {
        let rgb = tint.map_or(rgb, |t| t.at(corners[k][0], corners[k][2]));
        let a = AO_CURVE[usize::from(f.ao[k])];
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
    let target = if f.water {
        &mut out.water
    } else {
        &mut out.opaque
    };
    target.push_quad(corners, normal, colors, flip);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::AIR;
    use crate::generate::ColumnSet;
    use crate::terrain::tests::test_terrain;
    use proptest::prelude::*;

    /// The per-cell definition (the pre-M7 implementation), reference of the bit-mask mesher.
    fn mesh_reference(p: &PaddedChunk, blocks: &Blocks, tint: &ChunkTint) -> ChunkMesh {
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
                                blocks.linear(f.block),
                                (tint.grass == Some(f.block)).then_some(tint),
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

    fn ids() -> (Blocks, BlockId, BlockId) {
        let b = Blocks::builtin();
        let (stone, water) = (b.ids.granite, b.ids.water);
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

    #[test]
    fn generated_terrain_meshes() {
        let t = test_terrain();
        let col = ColumnSet::new(&t, 7, 7);
        let total: usize = col
            .chunk_range(1)
            .map(|cy| {
                mesh_chunk(&col.padded_chunk(&t, cy), t.blocks(), &ChunkTint::NONE)
                    .opaque
                    .quad_count()
            })
            .sum();
        assert!(total > 0);
    }

    #[test]
    fn same_mesh_as_the_reference_on_generated_terrain() {
        // Columns of the test world, land and sea, with their grass and water tint.
        let t = test_terrain();
        let mut meshed = 0;
        for (cx, cz) in [(7, 7), (0, 0), (30, 12), (60, 60), (100, 3)] {
            let col = ColumnSet::new(&t, cx, cz);
            let tint = t.chunk_tint(cx, cz);
            for cy in col.chunk_range(1) {
                let p = col.padded_chunk(&t, cy);
                let m = mesh_chunk(&p, t.blocks(), &tint);
                assert_eq!(
                    m,
                    mesh_reference(&p, t.blocks(), &tint),
                    "chunk {cx} {cy} {cz}"
                );
                meshed += usize::from(!m.is_empty());
            }
        }
        assert!(meshed > 0);
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
            let b = Blocks::builtin();
            let ids = b.ids;
            let palette = [
                AIR,
                ids.granite,
                ids.water,
                ids.grass,
                ids.snow_layer.expect("snow layer"),
                ids.spruce_leaves.expect("leaves"),
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
            tint.grass = Some(ids.grass);
            for (k, d) in tint.water_depth.iter_mut().enumerate() {
                *d = (seed as usize + k) as u8 % 7;
            }
            prop_assert_eq!(mesh_chunk(&p, &b, &tint), mesh_reference(&p, &b, &tint));
        }
    }

    #[test]
    fn uniform_water_has_no_mesh() {
        let (b, _, water) = ids();
        assert!(mesh_chunk(&padded(|_, _, _| water), &b, &ChunkTint::NONE).is_empty());
    }
}
