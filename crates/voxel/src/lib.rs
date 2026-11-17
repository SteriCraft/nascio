//! Block registry, 32³ chunks, chunk generation and meshing (spec §5).
//!
//! Engine-agnostic: meshes are plain vertex arrays that the viewer converts.

pub mod block;
pub mod chunk;
pub mod climate;
pub mod delta;
pub mod fingerprint;
pub mod generate;
pub mod geology;
pub mod mesh;
pub mod rivers;
pub mod section;
pub mod terrain;
pub mod vegetation;

pub use chunk::{Chunk, ChunkDeltas, NoDeltas, PaddedChunk};
pub use delta::DeltaStore;
pub use fingerprint::{DEV_OVERRIDE_MARK, GenConfig};
pub use generate::{ColumnSet, chunk_at, generate_chunk};
pub use mesh::{ChunkMesh, MeshData, mesh_chunk};
pub use terrain::{ChunkTint, Terrain, VoxelConfig};
