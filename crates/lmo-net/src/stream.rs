//! The client's `BinaryStream` serialization: big-endian scalars, strings prefixed with a
//! u16 length. Strings of 0x8000 bytes or more are written as `ffff 01ff` followed by a
//! u32 length (seen in `UniverseAgentSender::InitiateAuthentication`).

use bytes::{BufMut, Bytes, BytesMut};

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("unexpected end of stream reading {what} at offset {offset}")]
    Eof { what: &'static str, offset: usize },
    #[error("invalid utf-8 string at offset {0}")]
    Utf8(usize),
}

pub type Result<T> = std::result::Result<T, StreamError>;

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    pub fn bytes(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
        if self.buf.len() - self.pos < n {
            return Err(StreamError::Eof { what, offset: self.pos });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N]> {
        Ok(self.bytes(N, what)?.try_into().unwrap())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>("u8")?[0])
    }
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array("u16")?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array("u32")?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.array("i32")?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array("u64")?))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_be_bytes(self.array("f32")?))
    }

    pub fn string_bytes(&mut self) -> Result<&'a [u8]> {
        let mut len = self.u16()? as usize;
        if len == 0xffff {
            let _marker = self.u16()?;
            len = self.u32()? as usize;
        }
        self.bytes(len, "string")
    }

    pub fn string(&mut self) -> Result<String> {
        let at = self.pos;
        let b = self.string_bytes()?;
        String::from_utf8(b.to_vec()).map_err(|_| StreamError::Utf8(at))
    }
}

#[derive(Default)]
pub struct Writer {
    buf: BytesMut,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn u8(mut self, v: u8) -> Self {
        self.buf.put_u8(v);
        self
    }
    pub fn bool(self, v: bool) -> Self {
        self.u8(v as u8)
    }
    pub fn u16(mut self, v: u16) -> Self {
        self.buf.put_u16(v);
        self
    }
    pub fn u32(mut self, v: u32) -> Self {
        self.buf.put_u32(v);
        self
    }
    pub fn i32(mut self, v: i32) -> Self {
        self.buf.put_i32(v);
        self
    }
    pub fn u64(mut self, v: u64) -> Self {
        self.buf.put_u64(v);
        self
    }
    pub fn f32(mut self, v: f32) -> Self {
        self.buf.put_f32(v);
        self
    }
    pub fn raw(mut self, b: &[u8]) -> Self {
        self.buf.put_slice(b);
        self
    }
    pub fn string_bytes(mut self, b: &[u8]) -> Self {
        if b.len() >= 0x8000 {
            self.buf.put_u16(0xffff);
            self.buf.put_u16(0x01ff);
            self.buf.put_u32(b.len() as u32);
        } else {
            self.buf.put_u16(b.len() as u16);
        }
        self.buf.put_slice(b);
        self
    }
    pub fn string(self, s: &str) -> Self {
        self.string_bytes(s.as_bytes())
    }
    pub fn finish(self) -> Bytes {
        self.buf.freeze()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_roundtrip() {
        let b = Writer::new().string("abc").u32(7).finish();
        assert_eq!(&b[..], b"\x00\x03abc\x00\x00\x00\x07");
        let mut r = Reader::new(&b);
        assert_eq!(r.string().unwrap(), "abc");
        assert_eq!(r.u32().unwrap(), 7);
    }

    #[test]
    fn long_string() {
        let s = "x".repeat(0x9000);
        let b = Writer::new().string(&s).finish();
        assert_eq!(&b[..8], &[0xff, 0xff, 0x01, 0xff, 0, 0, 0x90, 0]);
        assert_eq!(Reader::new(&b).string().unwrap(), s);
    }
}
