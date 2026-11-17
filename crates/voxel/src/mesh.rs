//! Greedy meshing of a padded chunk, with per-vertex ambient occlusion.
//!
//! Engine-agnostic output: plain vertex arrays, positions relative to the chunk origin.
//! Faces are merged when they share block and corner occlusion (the classic "greedy" pass), and
//! quads are split along the diagonal that keeps occlusion gradients isotropic.

use crate::block::{AIR, BlockId, Blocks, Matter};
use crate::chunk::{N, PaddedChunk};
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

/// Face drawn on `block` when its neighbour across the face is `neighbour`.
#[inline]
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

/// Greedy mesh of the inner 32³ of a padded chunk, grass tinted by `tint`.
pub fn mesh_chunk(p: &PaddedChunk, blocks: &Blocks, tint: &ChunkTint) -> ChunkMesh {
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
    use crate::chunk::NP;
    use crate::generate::ColumnSet;
    use crate::terrain::tests::test_terrain;

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
}
