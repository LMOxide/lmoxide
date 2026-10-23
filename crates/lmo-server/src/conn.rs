//! Shared TCP plumbing: accept loop and a framed RPC connection.

use std::{future::Future, net::SocketAddr, sync::Arc};

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use lmo_net::{Endpoint, Frame, FrameCodec, Rpc, proto::ProtoEndpoint};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;
use tracing::{Instrument, debug, info, info_span, warn};

use crate::Config;

pub struct Conn {
    framed: Framed<TcpStream, FrameCodec>,
    /// Our endpoint, used as the caller in outgoing RPCs.
    pub local: Endpoint,
    /// The client's endpoint as it identified itself (caller of its last RPC).
    pub remote: Option<ProtoEndpoint>,
}

impl Conn {
    /// Next RPC from the client, or `None` on disconnect.
    pub async fn recv(&mut self) -> anyhow::Result<Option<Rpc>> {
        let body = loop {
            match self.framed.next().await.transpose()? {
                None => return Ok(None),
                Some(Frame::Rpc(body)) => break body,
                Some(Frame::Flagged(raw)) => {
                    warn!(hex = %hex(&raw), "control frame (bit 31 set), ignored");
                }
            }
        };
        debug!(len = body.len(), hex = %hex(&body), "frame in");
        let rpc = Rpc::decode(body)?;
        info!(
            caller = ?rpc.header.caller_id,
            args = rpc.args.len(),
            "<- {}",
            rpc.describe()
        );
        self.remote = Some(rpc.header.caller_id);
        Ok(Some(rpc))
    }

    /// Calls `method` on the client's `target` endpoint.
    pub async fn send(&mut self, target: Endpoint, method: u32, args: Bytes) -> anyhow::Result<()> {
        self.send_as(None, target, method, args).await
    }

    /// Like [`Conn::send`], identifying our endpoint with `caller_instance` (some handlers
    /// check it, e.g. `AckAuthenticate` requires it to equal the player instance).
    pub async fn send_as(
        &mut self,
        caller_instance: Option<u32>,
        target: Endpoint,
        method: u32,
        args: Bytes,
    ) -> anyhow::Result<()> {
        let instance = self.remote.and_then(|r| r.instance);
        let rpc = Rpc::new(self.local.proto(caller_instance), target.proto(instance), method, args);
        info!(args = rpc.args.len(), "-> {}", rpc.describe());
        let body = rpc.encode();
        debug!(hex = %hex(&body), "frame out");
        self.framed.send(Frame::Rpc(body)).await?;
        Ok(())
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub async fn listen<F, Fut>(
    name: &'static str,
    addr: SocketAddr,
    cfg: Arc<Config>,
    handler: F,
) -> anyhow::Result<()>
where
    F: Fn(Conn, Arc<Config>) -> Fut + Copy + Send + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send,
{
    let listener = TcpListener::bind(addr).await?;
    info!("{name} listening on {addr}");
    loop {
        let (sock, peer) = listener.accept().await?;
        let cfg = cfg.clone();
        let span = info_span!("conn", %name, %peer);
        tokio::spawn(
            async move {
                info!("connected");
                sock.set_nodelay(true).ok();
                let conn = Conn {
                    framed: Framed::new(sock, FrameCodec),
                    local: Endpoint::UniverseAgent,
                    remote: None,
                };
                match handler(conn, cfg).await {
                    Ok(()) => info!("disconnected"),
                    Err(e) => warn!("connection error: {e:#}"),
                }
            }
            .instrument(span),
        );
    }
}
