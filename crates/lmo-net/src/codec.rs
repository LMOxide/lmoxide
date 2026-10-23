//! TCP framing: `[u32 BE length][i32 BE crc32(body)][body]`, where `length = 4 + body.len()`.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame(pub Bytes);

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame length {0} out of range")]
    BadLength(usize),
    #[error("crc mismatch: header {header:08x}, computed {computed:08x}")]
    Crc { header: u32, computed: u32 },
}

#[derive(Debug, Default)]
pub struct FrameCodec;

impl Decoder for FrameCodec {
    type Item = Frame;
    type Error = CodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, CodecError> {
        if src.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes(src[..4].try_into().unwrap()) as usize;
        if !(4..=MAX_FRAME).contains(&len) {
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
        Ok(Some(Frame(body)))
    }
}

impl Encoder<Frame> for FrameCodec {
    type Error = CodecError;

    fn encode(&mut self, frame: Frame, dst: &mut BytesMut) -> Result<(), CodecError> {
        dst.reserve(8 + frame.0.len());
        dst.put_u32(frame.0.len() as u32 + 4);
        dst.put_u32(crc32fast::hash(&frame.0));
        dst.put_slice(&frame.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut buf = BytesMut::new();
        FrameCodec.encode(Frame(Bytes::from_static(b"hello")), &mut buf).unwrap();
        assert_eq!(&buf[..4], &9u32.to_be_bytes());
        let f = FrameCodec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(&f.0[..], b"hello");
        assert!(buf.is_empty());
    }

    #[test]
    fn partial() {
        let mut full = BytesMut::new();
        FrameCodec.encode(Frame(Bytes::from_static(b"abc")), &mut full).unwrap();
        let mut part = BytesMut::from(&full[..6]);
        assert!(FrameCodec.decode(&mut part).unwrap().is_none());
    }
}
