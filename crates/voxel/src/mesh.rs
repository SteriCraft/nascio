//! Greedy meshing of a padded chunk, with per-vertex ambient occlusion.
//!
//! Engine-agnostic output: plain vertex arrays, positions relative to the chunk origin.
//! Faces are merged when they share block and corner occlusion (the classic "greedy" pass), and
//! quads are split along the diagonal that keeps occlusion gradients isotropic.

use crate::block::{AIR, BlockId, Matter, WATER, def, linear_color};
use crate::chunk::{N, PaddedChunk};

/// Brightness factor per occlusion level (0 = fully occluded corner, 3 = open).
const AO_CURVE: [f32; 4] = [0.45, 0.65, 0.82, 1.0];
/// Opacity of the water surface.
pub const WATER_ALPHA: f32 = 0.62;

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
fn face(block: BlockId, neighbour: BlockId) -> Option<bool> {
    match (def(block).matter, def(neighbour).matter) {
        (Matter::Solid, Matter::Empty | Matter::Fluid) => Some(false),
        (Matter::Fluid, Matter::Empty) => Some(true),
        _ => None,
    }
}

/// Mask entry: block and corner occlusion of one unit face.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Face {
    block: BlockId,
    water: bool,
    ao: [u8; 4],
}

/// Greedy mesh of the inner 32³ of a padded chunk.
pub fn mesh_chunk(p: &PaddedChunk) -> ChunkMesh {
    let mut out = ChunkMesh::default();
    if p.is_uniform(AIR) || p.blocks.iter().all(|&b| def(b).matter == Matter::Solid) {
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
                        mask[a as usize + N * b as usize] = face(block, neighbour).map(|water| {
                            let ao = if water {
                                [3; 4]
                            } else {
                                corner_ao(p, nb, u, v)
                            };
                            Face { block, water, ao }
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
                        emit(&mut out, f, d, u, v, sign, plane, normal, [a, b, w, h]);
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
fn corner_ao(p: &PaddedChunk, nb: [i32; 3], u: usize, v: usize) -> [u8; 4] {
    let solid = |du: i32, dv: i32| {
        let mut c = nb;
        c[u] += du;
        c[v] += dv;
        u8::from(def(p.get(c[0], c[1], c[2])).matter == Matter::Solid)
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
    let rgb = linear_color(f.block);
    let alpha = if f.block == WATER { WATER_ALPHA } else { 1.0 };
    let mut colors = f.ao.map(|o| {
        let k = AO_CURVE[usize::from(o)];
        [rgb[0] * k, rgb[1] * k, rgb[2] * k, alpha]
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
    use crate::block::{STONE, WATER};
    use crate::chunk::NP;
    use crate::generate::ColumnSet;
    use crate::terrain::tests::test_terrain;

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
        let p = padded(|_, y, _| if y < 10 { STONE } else { AIR });
        let m = mesh_chunk(&p);
        assert_eq!(m.opaque.quad_count(), 1);
        assert!(m.water.is_empty());
        assert_eq!(m.opaque.normals[0], [0.0, 1.0, 0.0]);
        assert!(m.opaque.positions.iter().all(|p| p[1] == 10.0));
    }

    #[test]
    fn single_block_has_six_faces_facing_out() {
        let p = padded(|x, y, z| if (x, y, z) == (4, 5, 6) { STONE } else { AIR });
        let m = mesh_chunk(&p);
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
        let p = padded(|_, y, _| match y {
            ..3 => STONE,
            3..20 => WATER,
            _ => AIR,
        });
        let m = mesh_chunk(&p);
        assert_eq!(m.water.quad_count(), 1);
        assert_eq!(m.opaque.quad_count(), 1);
        assert!(m.water.positions.iter().all(|p| p[1] == 20.0));
    }

    #[test]
    fn buried_and_empty_chunks_have_no_mesh() {
        assert!(mesh_chunk(&padded(|_, _, _| STONE)).is_empty());
        assert!(mesh_chunk(&padded(|_, _, _| AIR)).is_empty());
    }

    #[test]
    fn generated_terrain_meshes() {
        let t = test_terrain();
        let col = ColumnSet::new(&t, 7, 7);
        let total: usize = col
            .chunk_range(1)
            .map(|cy| mesh_chunk(&col.padded_chunk(&t, cy)).opaque.quad_count())
            .sum();
        assert!(total > 0);
    }
}
