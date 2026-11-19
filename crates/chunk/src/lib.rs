//! Chunk types shared by the server and the client, without any generation code: the block
//! table of a world, 32³ chunks, the grass tint of a chunk column, and meshing.
//!
//! The client depends on this crate and never on `nascio-world`, `nascio-worldgen` or
//! `nascio-voxel`: the compiler guarantees it cannot generate terrain (invariant 5).

pub mod block;
pub mod chunk;
pub mod horizon;
pub mod mesh;
pub mod tint;

pub use block::{
    AIR, BlockDef, BlockId, BlockRegistry, BlockTextures, Blocks, FaceTexture, FaceTint, Matter,
};
pub use chunk::{Chunk, PaddedChunk};
pub use mesh::{ChunkMesh, FoliageMode, MeshData, mesh_chunk, mesh_chunk_with};
pub use tint::ChunkTint;
