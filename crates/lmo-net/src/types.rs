//! Engine value types as they appear in `BinaryStream` arguments.

use crate::stream::{self, Reader, Writer};

/// `ID32`: a (type, instance) pair, streamed as two big-endian u32s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Id32 {
    pub kind: u32,
    pub instance: u32,
}

impl Id32 {
    pub const fn new(kind: u32, instance: u32) -> Self {
        Self { kind, instance }
    }

    pub fn read(r: &mut Reader) -> stream::Result<Self> {
        Ok(Self { kind: r.u32()?, instance: r.u32()? })
    }

    pub fn write(self, w: Writer) -> Writer {
        w.u32(self.kind).u32(self.instance)
    }
}

/// `universe::LoginStatus`, the login progress/error codes shown by the client.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginStatus {
    ErrorNoUsernameOrPassword = 0,
    ConnectingToUniverse,
    ConnectedToUniverse,
    ErrorFailedToConnectToUniverse,
    AuthenticatingWithUniverse,
    UniverseAuthenticationOk,
    UniverseAuthenticationFailed,
    ConnectingToTerritory,
    ConnectedToTerritory,
    ErrorFailedToConnectToTerritory,
    AuthenticatingWithTerritory,
    TerritoryAuthenticationOk,
    ErrorTerritoryAuthenticationFailed,
    RequestingCharacterAndDimensionLists,
    GotCharacterList,
    GotDimensionList,
    ErrorLoginProblem,
    ConnectingToGameserver,
    ConnectedToGameserver,
    ErrorFailedToConnectToGameserver,
}

