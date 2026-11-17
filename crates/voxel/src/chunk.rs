//! 32³ chunks and the `generate ⊕ delta` interface (spec §5).

use nascio_core::consts::CHUNK_SIZE;
use nascio_core::coords::{ChunkPos, LocalPos};

use crate::block::{AIR, BlockId};

/// Side of a chunk, as `usize`.
pub const N: usize = CHUNK_SIZE as usize;
/// Blocks per chunk.
pub const VOLUME: usize = N * N * N;
/// Side of a chunk with a one-block border on every face (for meshing).
pub const NP: usize = N + 2;

/// A chunk of 32³ blocks, indexed by [`LocalPos::index`] (`x` fastest, then `z`, then `y`).
#[derive(Clone, PartialEq, Eq)]
pub struct Chunk {
    blocks: Box<[BlockId; VOLUME]>,
}

impl Chunk {
    /// A chunk full of one block.
    pub fn filled(id: BlockId) -> Self {
        Self {
            blocks: Box::new([id; VOLUME]),
        }
    }

    #[inline]
    pub fn get(&self, p: LocalPos) -> BlockId {
        self.blocks[p.index()]
    }

    #[inline]
    pub fn set(&mut self, p: LocalPos, id: BlockId) {
        self.blocks[p.index()] = id;
    }

    pub fn blocks(&self) -> &[BlockId; VOLUME] {
        &self.blocks
    }

    pub fn blocks_mut(&mut self) -> &mut [BlockId; VOLUME] {
        &mut self.blocks
    }

    /// Whether every block is `id`.
    pub fn is_uniform(&self, id: BlockId) -> bool {
        self.blocks.iter().all(|&b| b == id)
    }
}

impl Default for Chunk {
    fn default() -> Self {
        Self::filled(AIR)
    }
}

impl std::fmt::Debug for Chunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Chunk({} non-air blocks)", {
            self.blocks.iter().filter(|&&b| b != AIR).count()
        })
    }
}

/// A chunk plus a one-block border taken from its neighbours: `NP³` blocks, local coordinates
/// `-1..=32` stored at `0..34`, `x` fastest then `z` then `y`.
#[derive(Clone)]
pub struct PaddedChunk {
    pub blocks: Box<[BlockId; NP * NP * NP]>,
}

impl PaddedChunk {
    /// Index of local coordinates in `-1..=32`.
    #[inline]
    pub fn index(x: i32, y: i32, z: i32) -> usize {
        let (x, y, z) = ((x + 1) as usize, (y + 1) as usize, (z + 1) as usize);
        x + NP * (z + NP * y)
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.blocks[Self::index(x, y, z)]
    }

    /// A padded chunk filled with `id`.
    pub fn filled(id: BlockId) -> Self {
        Self {
            blocks: Box::new([id; NP * NP * NP]),
        }
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        self.blocks[Self::index(x, y, z)] = id;
    }

    /// The inner 32³ chunk.
    pub fn inner(&self) -> Chunk {
        let mut c = Chunk::filled(AIR);
        let dst = c.blocks_mut();
        for y in 0..N {
            for z in 0..N {
                for x in 0..N {
                    dst[x + N * (z + N * y)] = self.get(x as i32, y as i32, z as i32);
                }
            }
        }
        c
    }

    /// Whether every block (border included) is `id`.
    pub fn is_uniform(&self, id: BlockId) -> bool {
        self.blocks.iter().all(|&b| b == id)
    }
}

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
