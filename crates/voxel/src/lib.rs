//! Block registry, 32³ chunks, chunk generation and meshing (spec §5).
//!
//! Engine-agnostic: meshes are plain vertex arrays that the viewer converts.

pub mod block;
pub mod chunk;
pub mod climate;
pub mod generate;
pub mod geology;
pub mod mesh;
pub mod rivers;
pub mod section;
pub mod terrain;

pub use chunk::{Chunk, ChunkDeltas, NoDeltas, PaddedChunk};
pub use generate::{ColumnSet, chunk_at, generate_chunk};
pub use mesh::{ChunkMesh, MeshData, mesh_chunk};
pub use terrain::{ChunkTint, Terrain, VoxelConfig};
