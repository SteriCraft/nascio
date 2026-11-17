//! Frames on a byte stream: a varint length, then the payload. [`Deframer`] cuts the bytes
//! received in any pieces back into frames.

use anyhow::ensure;

use crate::varint::{Reader, put};

/// Largest control frame.
pub const MAX_CONTROL_FRAME: usize = 1 << 20;

/// A frame: length then payload.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    put(&mut out, payload.len() as u64);
    out.extend_from_slice(payload);
    out
}

/// Accumulates received bytes and yields complete frames.
#[derive(Debug)]
pub struct Deframer {
    buf: Vec<u8>,
    max: usize,
}

impl Deframer {
    /// Frames longer than `max` bytes are an error (the peer is then dropped).
    pub fn new(max: usize) -> Self {
        Self {
            buf: Vec::new(),
            max,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame, if any.
    pub fn next_frame(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        let mut r = Reader::new(&self.buf);
        let len = match r.var() {
            Ok(len) => len as usize,
            // An incomplete length: wait (at most 10 bytes, else malformed).
            Err(_) if self.buf.len() < 10 => return Ok(None),
            Err(e) => return Err(e),
        };
        ensure!(len <= self.max, "frame of {len} bytes above {}", self.max);
        let head = self.buf.len() - r.rest().len();
        if r.rest().len() < len {
            return Ok(None);
        }
        let f = self.buf[head..head + len].to_vec();
        self.buf.drain(..head + len);
        Ok(Some(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn frames_survive_any_cut(
            payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..300), 0..8),
            cut in 1usize..50,
        ) {
            let stream: Vec<u8> = payloads.iter().flat_map(|p| frame(p)).collect();
            let mut d = Deframer::new(1_000);
            let mut got = Vec::new();
            for piece in stream.chunks(cut) {
                d.push(piece);
                while let Some(f) = d.next_frame().unwrap() {
                    got.push(f);
                }
            }
            prop_assert_eq!(got, payloads);
        }
    }

    #[test]
    fn oversized_frames_are_refused() {
        let mut d = Deframer::new(10);
        d.push(&frame(&[0; 11]));
        assert!(d.next_frame().is_err());
        let mut d = Deframer::new(10);
        d.push(&[0xff; 12]);
        assert!(d.next_frame().is_err());
    }
}
