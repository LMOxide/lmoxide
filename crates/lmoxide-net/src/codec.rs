//! TCP framing: `[u32 BE length][i32 BE crc32(body)][body]`, where `length = 4 + body.len()`.
//!
//! Bit 31 of the length marks a control frame: same `[crc32][body]` layout, but the body is not
//! an RPC. The client sends one with body `01` right after game-server authentication.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A normal CRC-checked frame body.
    Rpc(Bytes),
    /// A control frame (bit 31 of the length set), CRC-checked like [`Frame::Rpc`].
    Flagged(Bytes),
}

const FLAG: usize = 0x8000_0000;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame length {0} out of range")]
    BadLength(usize),
    #[error("crc mismatch: header {header:08x}, computed {computed:08x}")]
    Crc { header: u32, computed: u32 },
}

/// Frames up to a maximum length (both directions; [`FrameCodec::default`] allows 16 MiB). A
/// server facing untrusted clients should use a smaller limit: a length prefix makes it
/// reserve that much memory before the body arrives.
#[derive(Debug, Clone, Copy)]
pub struct FrameCodec {
    max: usize,
}

impl FrameCodec {
    pub const fn new(max_frame: usize) -> Self {
        Self { max: max_frame }
    }
}

impl Default for FrameCodec {
    fn default() -> Self {
        Self::new(MAX_FRAME)
    }
}

impl Decoder for FrameCodec {
    type Item = Frame;
    type Error = CodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, CodecError> {
        if src.len() < 4 {
            return Ok(None);
        }
        let raw = u32::from_be_bytes(src[..4].try_into().unwrap()) as usize;
        let flagged = raw & FLAG != 0;
        let len = raw & !FLAG;
        if !(4..=self.max).contains(&len) {
            return Err(CodecError::BadLength(len));
        }
        if src.len() < 4 + len {
            src.reserve(4 + len - src.len());
            return Ok(None);
        }
        src.advance(4);
        let header = src.get_u32();
        let body = src.split_to(len - 4).freeze();
        let computed = crc32fast::hash(&body);
        if computed != header {
            return Err(CodecError::Crc { header, computed });
        }
        Ok(Some(if flagged { Frame::Flagged(body) } else { Frame::Rpc(body) }))
    }
}

impl Encoder<Frame> for FrameCodec {
    type Error = CodecError;

    fn encode(&mut self, frame: Frame, dst: &mut BytesMut) -> Result<(), CodecError> {
        let (body, flag) = match frame {
            Frame::Rpc(body) => (body, 0),
            Frame::Flagged(body) => (body, FLAG),
        };
        dst.reserve(8 + body.len());
        dst.put_u32(((body.len() + 4) | flag) as u32);
        dst.put_u32(crc32fast::hash(&body));
        dst.put_slice(&body);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut buf = BytesMut::new();
        FrameCodec::default().encode(Frame::Rpc(Bytes::from_static(b"hello")), &mut buf).unwrap();
        assert_eq!(&buf[..4], &9u32.to_be_bytes());
        let f = FrameCodec::default().decode(&mut buf).unwrap().unwrap();
        assert_eq!(f, Frame::Rpc(Bytes::from_static(b"hello")));
        assert!(buf.is_empty());
    }

    #[test]
    fn flagged() {
        // Captured from the client after game-server authentication.
        let mut buf = BytesMut::from(&[0x80, 0, 0, 5, 0xa5, 0x05, 0xdf, 0x1b, 0x01, 0xff][..]);
        let f = FrameCodec::default().decode(&mut buf).unwrap().unwrap();
        assert_eq!(f, Frame::Flagged(Bytes::from_static(&[0x01])));
        assert_eq!(&buf[..], &[0xff]);
    }

    #[test]
    fn partial() {
        let mut full = BytesMut::new();
        FrameCodec::default().encode(Frame::Rpc(Bytes::from_static(b"abc")), &mut full).unwrap();
        let mut part = BytesMut::from(&full[..6]);
        assert!(FrameCodec::default().decode(&mut part).unwrap().is_none());
    }

    #[test]
    fn rejects_oversized() {
        let mut buf = BytesMut::from(&[0, 0, 0x10, 0, 0, 0, 0, 0][..]);
        assert!(matches!(FrameCodec::new(1024).decode(&mut buf), Err(CodecError::BadLength(4096))));
    }
}
