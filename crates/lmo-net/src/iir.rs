//! IIRs ("info items"): the game server's state updates, sent inside
//! `GameserverToClientRPC::PassBlob`. A blob is a sequence of IIRs; each is
//! (`n3InfoItemRemote::Construct`, client 0x7d6b20):
//!
//! ```text
//! u32   type id       Java-style hash of the registered name (data/iir_types.json)
//! ID32  identity      the dynel the IIR is about
//! bool  pass on       "isToBePassedOn"
//! [u32 len, protobuf] only for IIR classes with an embedded message
//! ...                 class-specific ReadSubClass data
//! ```

use bytes::Bytes;

use crate::{rpc::endpoint_hash, stream::Writer, types::Id32};

/// IIR type id for a registered IIR name (e.g. `"PlayfieldFIIR_t"`).
pub const fn type_id(name: &str) -> u32 {
    endpoint_hash(name)
}

/// Builds one IIR record. `body` writes the protobuf (if the class has one) and the
/// class-specific fields.
pub fn record(name: &str, identity: Id32, pass_on: bool, body: impl FnOnce(Writer) -> Writer) -> Bytes {
    let w = identity.write(Writer::new().u32(type_id(name))).bool(pass_on);
    body(w).finish()
}

/// Concatenates IIR records into a `PassBlob` argument (BinaryStream: u32 length + bytes).
pub fn pass_blob(records: &[Bytes]) -> Bytes {
    let total: usize = records.iter().map(|r| r.len()).sum();
    let w = records.iter().fold(Writer::new().u32(total as u32), |w, r| w.raw(r));
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_ids_match_extracted_registry() {
        // values from data/iir_types.json (extracted from the client's registration code)
        assert_eq!(type_id("PlayfieldFIIR_t"), 0x5d85bfc7);
        assert_eq!(type_id("CharInPlayIIR_t"), 0x864cfef8);
    }

    #[test]
    fn record_layout() {
        let r = record("CharInPlayIIR_t", Id32::new(50000, 7), false, |w| w);
        assert_eq!(&r[..], &[0x86, 0x4c, 0xfe, 0xf8, 0, 0, 0xc3, 0x50, 0, 0, 0, 7, 0]);
        let blob = pass_blob(&[r.clone(), r]);
        assert_eq!(&blob[..4], &26u32.to_be_bytes());
    }
}
