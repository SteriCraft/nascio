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
///
/// A chunk of one block (air, deep rock, open sea: most of the world) is stored as that block
/// alone; the 64 KiB array exists only once the chunk holds two kinds of block (audit F5).
/// [`Chunk::encode`] gives the compact form for the network and the disk (palette + runs).
#[derive(Clone)]
pub struct Chunk {
    storage: Storage,
}

#[derive(Clone)]
enum Storage {
    Uniform(BlockId),
    Dense(Box<[BlockId; VOLUME]>),
}

/// Version byte of [`Chunk::encode`].
const CODEC_VERSION: u8 = 1;

impl Chunk {
    /// A chunk full of one block (no allocation).
    pub fn filled(id: BlockId) -> Self {
        Self {
            storage: Storage::Uniform(id),
        }
    }

    /// A chunk from its blocks, compacted when they are all the same.
    pub fn from_blocks(blocks: Box<[BlockId; VOLUME]>) -> Self {
        let mut c = Self {
            storage: Storage::Dense(blocks),
        };
        c.compact();
        c
    }

    #[inline]
    pub fn get(&self, p: LocalPos) -> BlockId {
        match &self.storage {
            Storage::Uniform(id) => *id,
            Storage::Dense(b) => b[p.index()],
        }
    }

    /// Sets a block; a uniform chunk allocates its array only when the block changes.
    #[inline]
    pub fn set(&mut self, p: LocalPos, id: BlockId) {
        if matches!(self.storage, Storage::Uniform(u) if u == id) {
            return;
        }
        self.blocks_mut()[p.index()] = id;
    }

    /// The block filling the whole chunk, if there is one.
    pub fn uniform(&self) -> Option<BlockId> {
        match &self.storage {
            Storage::Uniform(id) => Some(*id),
            Storage::Dense(b) => b.iter().all(|&x| x == b[0]).then_some(b[0]),
        }
    }

    /// Whether every block is `id`.
    pub fn is_uniform(&self, id: BlockId) -> bool {
        self.uniform() == Some(id)
    }

    /// A copy of all the blocks, in index order.
    pub fn to_dense(&self) -> Box<[BlockId; VOLUME]> {
        match &self.storage {
            Storage::Uniform(id) => Box::new([*id; VOLUME]),
            Storage::Dense(b) => b.clone(),
        }
    }

    /// All the blocks (the array is allocated if the chunk was uniform).
    pub fn blocks_mut(&mut self) -> &mut [BlockId; VOLUME] {
        if let Storage::Uniform(id) = self.storage {
            self.storage = Storage::Dense(Box::new([id; VOLUME]));
        }
        match &mut self.storage {
            Storage::Dense(b) => b,
            Storage::Uniform(_) => unreachable!("made dense above"),
        }
    }

    /// Frees the array if every block is the same.
    pub fn compact(&mut self) {
        if let (Storage::Dense(_), Some(id)) = (&self.storage, self.uniform()) {
            self.storage = Storage::Uniform(id);
        }
    }

    /// Bytes on the heap: 0 for a uniform chunk, 64 KiB otherwise.
    pub fn heap_bytes(&self) -> usize {
        match self.storage {
            Storage::Uniform(_) => 0,
            Storage::Dense(_) => VOLUME * std::mem::size_of::<BlockId>(),
        }
    }

    /// Compact form: a version byte, the palette (`u16` count, then the ids), then runs of
    /// `(palette index, length)` in index order, all `u16` little-endian. A uniform chunk takes
    /// 9 bytes. To be used by the protocol and the snapshots (audit F5).
    pub fn encode(&self) -> Vec<u8> {
        let mut palette: Vec<BlockId> = Vec::new();
        let mut runs: Vec<(u16, u16)> = Vec::new();
        let mut push = |id: BlockId, len: usize| {
            let k = palette.iter().position(|&q| q == id).unwrap_or_else(|| {
                palette.push(id);
                palette.len() - 1
            });
            runs.push((k as u16, len as u16));
        };
        match &self.storage {
            Storage::Uniform(id) => push(*id, VOLUME),
            Storage::Dense(b) => {
                let mut start = 0;
                for i in 1..=VOLUME {
                    if i == VOLUME || b[i] != b[start] {
                        push(b[start], i - start);
                        start = i;
                    }
                }
            }
        }
        let mut out = Vec::with_capacity(3 + 2 * palette.len() + 4 * runs.len());
        out.push(CODEC_VERSION);
        out.extend_from_slice(&(palette.len() as u16).to_le_bytes());
        for id in &palette {
            out.extend_from_slice(&id.to_le_bytes());
        }
        for (k, len) in runs {
            out.extend_from_slice(&k.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
        }
        out
    }

    /// Inverse of [`Chunk::encode`]; refuses malformed or truncated input.
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        use anyhow::{Context, ensure};
        let u16_at = |i: usize| {
            bytes
                .get(i..i + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .context("chunk encoding truncated")
        };
        ensure!(
            bytes.first() == Some(&CODEC_VERSION),
            "unknown chunk encoding version"
        );
        let np = usize::from(u16_at(1)?);
        ensure!(np > 0, "chunk encoding has an empty palette");
        let palette: Vec<BlockId> = (0..np)
            .map(|k| u16_at(3 + 2 * k))
            .collect::<anyhow::Result<_>>()?;
        let runs = &bytes[3 + 2 * np..];
        ensure!(runs.len().is_multiple_of(4), "chunk encoding truncated");
        let mut blocks = Box::new([AIR; VOLUME]);
        let mut at = 0;
        for r in runs.as_chunks::<4>().0 {
            let k = usize::from(u16::from_le_bytes([r[0], r[1]]));
            let len = usize::from(u16::from_le_bytes([r[2], r[3]]));
            let id = *palette
                .get(k)
                .context("chunk encoding palette index out of range")?;
            ensure!(
                len > 0 && at + len <= VOLUME,
                "chunk encoding has the wrong number of blocks"
            );
            blocks[at..at + len].fill(id);
            at += len;
        }
        ensure!(
            at == VOLUME,
            "chunk encoding has the wrong number of blocks"
        );
        Ok(Self::from_blocks(blocks))
    }
}

impl PartialEq for Chunk {
    /// Same blocks, whatever the storage.
    fn eq(&self, o: &Self) -> bool {
        match (&self.storage, &o.storage) {
            (Storage::Uniform(a), Storage::Uniform(b)) => a == b,
            (Storage::Dense(a), Storage::Dense(b)) => a == b,
            (Storage::Uniform(a), Storage::Dense(b)) | (Storage::Dense(b), Storage::Uniform(a)) => {
                b.iter().all(|x| x == a)
            }
        }
    }
}

impl Eq for Chunk {}

impl Default for Chunk {
    fn default() -> Self {
        Self::filled(AIR)
    }
}

impl std::fmt::Debug for Chunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.storage {
            Storage::Uniform(id) => write!(f, "Chunk(uniform {id})"),
            Storage::Dense(b) => write!(
                f,
                "Chunk({} non-air blocks)",
                b.iter().filter(|&&x| x != AIR).count()
            ),
        }
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

    /// The inner 32³ chunk (compacted when uniform).
    pub fn inner(&self) -> Chunk {
        let mut dst = Box::new([AIR; VOLUME]);
        for y in 0..N {
            for z in 0..N {
                let src = Self::index(0, y as i32, z as i32);
                let at = N * (z + N * y);
                dst[at..at + N].copy_from_slice(&self.blocks[src..src + N]);
            }
        }
        Chunk::from_blocks(dst)
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn uniform_chunks_hold_no_array() {
        let mut c = Chunk::filled(7);
        assert_eq!(c.heap_bytes(), 0);
        c.set(LocalPos { x: 3, y: 4, z: 5 }, 7);
        assert_eq!(c.heap_bytes(), 0, "setting the same block keeps it uniform");
        c.set(LocalPos { x: 3, y: 4, z: 5 }, 9);
        assert_eq!(c.heap_bytes(), VOLUME * 2);
        assert_eq!(c.get(LocalPos { x: 3, y: 4, z: 5 }), 9);
        assert_eq!(c.get(LocalPos { x: 0, y: 0, z: 0 }), 7);
        assert_eq!(c.uniform(), None);
        c.set(LocalPos { x: 3, y: 4, z: 5 }, 7);
        assert_eq!(c, Chunk::filled(7), "equality ignores the storage");
        c.compact();
        assert_eq!(c.heap_bytes(), 0);
        assert_eq!(Chunk::filled(7).encode().len(), 9);
    }

    #[test]
    fn decode_refuses_malformed_input() {
        let good = Chunk::filled(3).encode();
        assert!(Chunk::decode(&good).is_ok());
        assert!(Chunk::decode(&[]).is_err());
        assert!(Chunk::decode(&good[..good.len() - 1]).is_err());
        let mut v = good.clone();
        v[0] = 99;
        assert!(Chunk::decode(&v).is_err(), "version");
        let mut v = good.clone();
        v[5] = 1;
        assert!(Chunk::decode(&v).is_err(), "palette index");
        let mut v = good;
        v[8] = 0x40;
        assert!(Chunk::decode(&v).is_err(), "block count");
    }

    proptest! {
        /// Random chunks of a few blocks in runs: decode(encode(c)) == c, and the encoding is
        /// smaller than the raw 64 KiB whenever the runs are long.
        #[test]
        fn encode_round_trips(
            runs in proptest::collection::vec((0u16..6, 1usize..4000), 1..40),
        ) {
            let mut blocks = Box::new([AIR; VOLUME]);
            let mut at = 0;
            for &(id, len) in runs.iter().cycle() {
                if at == VOLUME {
                    break;
                }
                let end = (at + len).min(VOLUME);
                blocks[at..end].fill(id);
                at = end;
            }
            let c = Chunk::from_blocks(blocks);
            let bytes = c.encode();
            prop_assert!(bytes.len() < VOLUME * 2);
            let d = Chunk::decode(&bytes).expect("decodes");
            prop_assert_eq!(d.heap_bytes(), c.heap_bytes());
            prop_assert_eq!(d, c);
        }
    }
}
