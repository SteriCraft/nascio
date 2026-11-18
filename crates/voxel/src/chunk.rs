//! 32³ chunks (types in `nascio-chunk`) and the `generate ⊕ delta` interface (spec §5).

use nascio_core::coords::ChunkPos;

pub use nascio_chunk::chunk::*;

/// Player modifications of the world (spec §5: `Chunk = generate(pos) ⊕ delta(pos)`).
/// Implemented by [`crate::delta::DeltaStore`].
pub trait ChunkDeltas {
    /// Applies the stored modifications of `pos`, if any, to a freshly generated chunk.
    fn apply(&self, pos: ChunkPos, chunk: &mut Chunk);
}

/// No modification anywhere: the pristine generated world.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoDeltas;

impl ChunkDeltas for NoDeltas {
    fn apply(&self, _pos: ChunkPos, _chunk: &mut Chunk) {}
}
