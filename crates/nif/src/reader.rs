//! Bounds-checked little-endian cursor; `base` keeps reported offsets file-absolute.

use crate::{Error, Result};

pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    base: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, pos: 0, base: 0 }
    }
    pub fn with_base(bytes: &'a [u8], base: usize) -> Self {
        Reader { bytes, pos: 0, base }
    }
    pub fn pos(&self) -> usize {
        self.base + self.pos
    }
    pub fn seek(&mut self, absolute: usize) {
        self.pos = absolute - self.base;
    }
    pub fn rest(&self) -> &'a [u8] {
        &self.bytes[self.pos.min(self.bytes.len())..]
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.bytes.get(self.pos..self.pos + n).ok_or(Error::Truncated { offset: self.pos(), what: "data" })?;
        self.pos += n;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.arr::<1>()?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.arr()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.arr()?))
    }
    pub fn vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    /// u32 length + bytes (no terminator).
    pub fn sized_string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        if n > self.bytes.len() {
            return Err(Error::Truncated { offset: self.pos(), what: "string length" });
        }
        Ok(latin1(self.bytes(n)?))
    }
    /// u8 length (including NUL) + bytes.
    pub fn export_string(&mut self) -> Result<String> {
        let n = self.u8()? as usize;
        let b = self.bytes(n)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        Ok(latin1(&b[..end]))
    }
}

pub(crate) fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}
