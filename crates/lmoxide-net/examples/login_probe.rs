//! Logs in to a universe agent like the client does and prints the answer: a quick check of
//! accounts and the login key without starting the game.
//!
//!   cargo run -p lmoxide-net --example login_probe -- 127.0.0.1:7100 <user> <password> <login public key>

use futures::{SinkExt, StreamExt};
use lmoxide_net::{
    Endpoint, Frame, FrameCodec, Reader, Rpc, Writer, login,
    methods::{universe_agent as agent, universe_interface as interface},
};
use tokio_util::codec::Framed;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, addr, user, password, key] = &args[..] else {
        return Err("usage: login_probe <host:port> <user> <password> <login public key>".into());
    };
    let mut conn = Framed::new(tokio::net::TcpStream::connect(addr).await?, FrameCodec::default());
    let call = |method, args| {
        Frame::Rpc(Rpc::new(Endpoint::UniverseInterface.proto(None), Endpoint::UniverseAgent.proto(None), method, args).encode())
    };
    conn.send(call(agent::INITIATE_AUTHENTICATION, Writer::new().string(user).u32(1).finish())).await?;
    loop {
        let Some(Frame::Rpc(body)) = conn.next().await.transpose()? else { return Err("disconnected".into()) };
        let rpc = Rpc::decode(body)?;
        let mut r = Reader::new(&rpc.args);
        match rpc.method() {
            interface::CHALLENGE => {
                let challenge = r.string()?;
                let answer = login::answer_challenge(key, user, &challenge, 1, password);
                conn.send(call(agent::ANSWER_CHALLENGE, Writer::new().string(&answer).finish())).await?;
            }
            interface::ACK_AUTHENTICATE => {
                let status = r.i32()?;
                let (kind, instance) = (r.u32()?, r.u32()?);
                let territory = r.string()?;
                let cookie = r.u32()?;
                let reason = r.u32()?;
                println!("status {status}, player {kind:#x}:{instance}, territory {territory:?}, cookie set: {}, reason {reason}", cookie != 0);
                return Ok(());
            }
            m => println!("unexpected method {m:#x}"),
        }
    }
}
