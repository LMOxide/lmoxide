//! Wire protocol for the LEGO Minifigures Online client (Funcom's Dreamworld/N3 engine).
//!
//! A TCP stream carries frames (see [`codec`]); each frame holds one RPC: a protobuf
//! [`proto::RpcHeader`] naming the target endpoint and method, followed by the method's
//! arguments serialized as a big-endian [`stream`] ("BinaryStream" in the client).

pub mod codec;
pub mod iir;
pub mod login;
pub mod rpc;
pub mod stream;
pub mod types;

/// Protobuf messages extracted from the client's embedded descriptors (see `proto/`).
pub mod proto {
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/_.rs"));
    pub mod brick_system_proto {
        include!(concat!(env!("OUT_DIR"), "/brick_system_proto.rs"));
    }
    pub mod knubot_actor_iir_proto {
        include!(concat!(env!("OUT_DIR"), "/knubot_actor_iir_proto.rs"));
    }
    pub mod spawn_client_npc_proto {
        include!(concat!(env!("OUT_DIR"), "/spawn_client_npc_proto.rs"));
    }
}

/// RPC method IDs and signatures extracted from the client (see `tools/re/extract_rpc.py`).
pub mod methods {
    include!(concat!(env!("OUT_DIR"), "/methods.rs"));

    /// Looks up the C++ signature for a method on an endpoint, for logging.
    pub fn signature(interface: &str, id: u32) -> Option<&'static str> {
        INTERFACES
            .iter()
            .find(|(name, _)| *name == interface)
            .and_then(|(_, table)| table.iter().find(|(m, _)| *m == id))
            .map(|(_, sig)| *sig)
    }
}

pub use codec::{Frame, FrameCodec};
pub use rpc::{Endpoint, Rpc};
pub use stream::{Reader, Writer};
