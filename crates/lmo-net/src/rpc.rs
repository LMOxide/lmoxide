//! RPC envelope: `[u32 BE header length][RpcHeader protobuf][BinaryStream arguments]`.

use bytes::{BufMut, Bytes, BytesMut};
use prost::Message;

use crate::proto::{ProtoEndpoint, RpcHeader};

/// Endpoint type IDs are a Java-style string hash (`h = h * 31 + c`) of the endpoint name.
pub const fn endpoint_hash(name: &str) -> u32 {
    let b = name.as_bytes();
    let mut h: u32 = 0;
    let mut i = 0;
    while i < b.len() {
        h = h.wrapping_mul(31).wrapping_add(b[i] as u32);
        i += 1;
    }
    h
}

/// Endpoint names. Client-side endpoints are `*Interface` (and `GameserverToClientRPC`);
/// the server-side endpoints the client calls are `*Agent` (and `ClientToGameserverRPC`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    UniverseAgent,
    UniverseInterface,
    PlayerAgent,
    PlayerInterface,
    CSPlayerAgent,
    CSPlayerInterface,
    CSPlayerProjectAgent,
    CSPlayerProjectInterface,
    ClientToGameserverRPC,
    GameserverToClientRPC,
    GameCharAgent,
    GameCharInterface,
    GameCharAgentProject,
    GameCharInterfaceProject,
    TradepostAgent,
    TradepostInterface,
    TradepostAgentProject,
    TradepostInterfaceProject,
}

impl Endpoint {
    pub const ALL: &[Endpoint] = &[
        Self::UniverseAgent,
        Self::UniverseInterface,
        Self::PlayerAgent,
        Self::PlayerInterface,
        Self::CSPlayerAgent,
        Self::CSPlayerInterface,
        Self::CSPlayerProjectAgent,
        Self::CSPlayerProjectInterface,
        Self::ClientToGameserverRPC,
        Self::GameserverToClientRPC,
        Self::GameCharAgent,
        Self::GameCharInterface,
        Self::GameCharAgentProject,
        Self::GameCharInterfaceProject,
        Self::TradepostAgent,
        Self::TradepostInterface,
        Self::TradepostAgentProject,
        Self::TradepostInterfaceProject,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::UniverseAgent => "UniverseAgent",
            Self::UniverseInterface => "UniverseInterface",
            Self::PlayerAgent => "PlayerAgent",
            Self::PlayerInterface => "PlayerInterface",
            Self::CSPlayerAgent => "CSPlayerAgent",
            Self::CSPlayerInterface => "CSPlayerInterface",
            Self::CSPlayerProjectAgent => "CSPlayerProjectAgent",
            Self::CSPlayerProjectInterface => "CSPlayerProjectInterface",
            Self::ClientToGameserverRPC => "ClientToGameserverRPC",
            Self::GameserverToClientRPC => "GameserverToClientRPC",
            Self::GameCharAgent => "GameCharAgent",
            Self::GameCharInterface => "GameCharInterface",
            Self::GameCharAgentProject => "GameCharAgentProject",
            Self::GameCharInterfaceProject => "GameCharInterfaceProject",
            Self::TradepostAgent => "TradepostAgent",
            Self::TradepostInterface => "TradepostInterface",
            Self::TradepostAgentProject => "TradepostAgentProject",
            Self::TradepostInterfaceProject => "TradepostInterfaceProject",
        }
    }

    pub fn type_id(self) -> u32 {
        endpoint_hash(self.name())
    }

    pub fn from_type_id(id: u32) -> Option<Self> {
        Self::ALL.iter().copied().find(|e| e.type_id() == id)
    }

    pub fn proto(self, instance: Option<u32>) -> ProtoEndpoint {
        ProtoEndpoint { r#type: Some(self.type_id()), instance, token: None }
    }
}

/// A decoded RPC.
#[derive(Debug, Clone)]
pub struct Rpc {
    pub header: RpcHeader,
    pub args: Bytes,
}

#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("rpc frame too short")]
    Short,
    #[error("bad rpc header: {0}")]
    Header(#[from] prost::DecodeError),
}

impl Rpc {
    pub fn new(caller: ProtoEndpoint, target: ProtoEndpoint, method: u32, args: Bytes) -> Self {
        Self {
            header: RpcHeader {
                caller_id: caller,
                endpoint_id: target,
                routing_destination: vec![],
                full_routing_context: vec![],
                method_instance: method,
            },
            args,
        }
    }

    pub fn decode(mut body: Bytes) -> Result<Self, RpcError> {
        if body.len() < 4 {
            return Err(RpcError::Short);
        }
        let hlen = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
        if body.len() < 4 + hlen {
            return Err(RpcError::Short);
        }
        let _ = body.split_to(4);
        let header = RpcHeader::decode(body.split_to(hlen))?;
        Ok(Self { header, args: body })
    }

    pub fn encode(&self) -> Bytes {
        let h = self.header.encode_to_vec();
        let mut out = BytesMut::with_capacity(4 + h.len() + self.args.len());
        out.put_u32(h.len() as u32);
        out.put_slice(&h);
        out.put_slice(&self.args);
        out.freeze()
    }

    pub fn target(&self) -> Option<Endpoint> {
        self.header.endpoint_id.r#type.and_then(Endpoint::from_type_id)
    }

    pub fn method(&self) -> u32 {
        self.header.method_instance
    }

    /// `Endpoint::Method(signature)` for logs, falling back to raw IDs.
    pub fn describe(&self) -> String {
        let t = self.header.endpoint_id.r#type.unwrap_or(0);
        match self.target() {
            Some(e) => match crate::methods::signature(e.name(), self.method()) {
                Some(sig) => format!("{}::{}", e.name(), sig),
                None => format!("{}::<unknown method 0x{:08x}>", e.name(), self.method()),
            },
            None => format!("<endpoint 0x{t:08x}>::<method 0x{:08x}>", self.method()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_lmo_redux() {
        // lmo-redux computed the same hash in TypeScript
        assert_eq!(endpoint_hash("UniverseAgent"), 0x604ddba0);
    }

    #[test]
    fn rpc_roundtrip() {
        let rpc = Rpc::new(
            Endpoint::UniverseAgent.proto(None),
            Endpoint::UniverseInterface.proto(None),
            0x5254424a,
            Bytes::from_static(b"\x00\x01x"),
        );
        let back = Rpc::decode(rpc.encode()).unwrap();
        assert_eq!(back.target(), Some(Endpoint::UniverseInterface));
        assert_eq!(back.describe(), "UniverseInterface::Challenge(const String& data)");
    }
}
