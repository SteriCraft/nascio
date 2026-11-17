//! LEB128 varints and zigzag signed integers.

use anyhow::{Context, ensure};

/// Appends `v` as a LEB128 varint (7 bits per byte, low bits first).
pub fn put(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Appends a signed integer, zigzag-encoded (small magnitudes take one byte).
pub fn put_i(out: &mut Vec<u8>, v: i64) {
    put(out, ((v << 1) ^ (v >> 63)) as u64);
}

/// Reads bytes in order; every read checks the bounds.
#[derive(Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// Bytes not read yet.
    pub fn rest(&self) -> &'a [u8] {
        &self.bytes[self.at..]
    }

    pub fn is_empty(&self) -> bool {
        self.at == self.bytes.len()
    }

    pub fn u8(&mut self) -> anyhow::Result<u8> {
        let b = *self.bytes.get(self.at).context("truncated")?;
        self.at += 1;
        Ok(b)
    }

    pub fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        ensure!(self.bytes.len() - self.at >= n, "truncated");
        let s = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(s)
    }

    /// A varint of at most 64 bits.
    pub fn var(&mut self) -> anyhow::Result<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            v |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return Ok(v);
            }
        }
        anyhow::bail!("varint too long")
    }

    /// A varint that must fit `max`.
    pub fn var_max(&mut self, max: u64) -> anyhow::Result<u64> {
        let v = self.var()?;
        ensure!(v <= max, "value {v} above {max}");
        Ok(v)
    }

    /// A zigzag signed varint.
    pub fn var_i(&mut self) -> anyhow::Result<i64> {
        let v = self.var()?;
        Ok((v >> 1) as i64 ^ -((v & 1) as i64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn round_trip(u in any::<u64>(), i in any::<i64>()) {
            let mut b = Vec::new();
            put(&mut b, u);
            put_i(&mut b, i);
            let mut r = Reader::new(&b);
            prop_assert_eq!(r.var().unwrap(), u);
            prop_assert_eq!(r.var_i().unwrap(), i);
            prop_assert!(r.is_empty());
        }
    }

    #[test]
    fn small_values_are_one_byte() {
        let mut b = Vec::new();
        put(&mut b, 127);
        put_i(&mut b, -64);
        put_i(&mut b, 63);
        assert_eq!(b.len(), 3);
        assert!(Reader::new(&[0x80; 11]).var().is_err());
        assert!(Reader::new(&[0x80]).var().is_err());
    }
}
