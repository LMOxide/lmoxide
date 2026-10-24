//! Chat server. The client speaks plain-text IRC to it (`net::protocol::IRCConnection`, CRLF
//! lines) after the territory sends `PlayerInterface::ChatServerConnectReady(address, cookie,
//! character)`; zlib compression is off via `ChatCompression=false` in LocalConfig.xml.
//!
//! Client: `PASS :<cookie>`, `NICK <name>-<PlayerID stat>`, `USER hepp ignore ignore :<name>`;
//! we answer `001` (welcome) and `PONG`. Vicinity ("Say") text does not come through here: the
//! client sends it to the game server (`GameCharAgent::VicinityChatMessage`) after the HTTP chat
//! filter (`/rest/chat_filter`, see http.rs) approves it, and shows other players' lines only
//! when they arrive as `:<nick>!u@<inst>.0.0 VICINITYCHAT <sender> <mood> :<text>` on this
//! connection. The sender echoes its own line locally.
//!
//! Channels are IRC channels. The chat window's groups come from joined channels named
//! `#<group>_D<dimension>_<n>` (e.g. `ChatGroup_General`, the default "Global" tab): the server
//! makes the client join General on login. Other lines arrive as `FILTEREDPRIVMSG <target>
//! [<hash>] :<text>` (after the HTTP filter) and are relayed as `PRIVMSG` to the channel's other
//! members.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{LazyLock, Mutex},
};

use futures::StreamExt;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tokio_util::codec::{FramedRead, LinesCodec};
use tracing::{Instrument, debug, info, info_span, warn};

/// account -> lines to send on its chat connection
static CLIENTS: LazyLock<Mutex<HashMap<u32, mpsc::UnboundedSender<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// channel name (lower case) -> member accounts
static CHANNELS: LazyLock<Mutex<HashMap<String, HashSet<u32>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Set by `--bot`: the test bot answers channel messages, so incoming chat can be seen with
/// a single client.
pub static BOT_ECHO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Longest line accepted from a client.
const MAX_LINE: usize = 8 * 1024;
/// How long a connection may take to log in (`PASS`).
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Channels every character is put in.
const DEFAULT_CHANNELS: &[&str] = &["#ChatGroup_General_D1_1"];

pub async fn listen(addr: SocketAddr, max_per_ip: u32) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("chat listening on {addr}");
    loop {
        let (sock, peer) = match listener.accept().await {
            Ok(a) => a,
            Err(e) => {
                warn!("chat: accept failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let Some(guard) = crate::security::connection(peer.ip(), max_per_ip) else {
            warn!(%peer, "chat: too many connections from this address, refused");
            continue;
        };
        tokio::spawn(
            async move {
                let _guard = guard;
                match handle(sock).await {
                    Ok(()) => info!("disconnected"),
                    Err(e) => warn!("chat connection error: {e:#}"),
                }
            }
            .instrument(info_span!("chat", %peer)),
        );
    }
}

async fn handle(sock: TcpStream) -> anyhow::Result<()> {
    let (rd, mut wr) = sock.into_split();
    let mut lines = FramedRead::new(rd, LinesCodec::new_with_max_length(MAX_LINE));
    let login_deadline = tokio::time::Instant::now() + LOGIN_TIMEOUT;
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let mut account = None;
    let mut nick = String::new();
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(login_deadline), if account.is_none() => {
                anyhow::bail!("no PASS within {LOGIN_TIMEOUT:?}");
            }
            line = lines.next() => {
                let Some(line) = line.transpose()? else { break };
                debug!(%line, "<-");
                let (cmd, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
                match cmd.to_ascii_uppercase().as_str() {
                    "PASS" => {
                        let cookie: u32 = rest.trim_start_matches(':').trim().parse().unwrap_or(0);
                        account = crate::state::or_log(crate::state::db().game_session(cookie), "session", None);
                        info!(?account, "chat login");
                        let Some(a) = account else {
                            warn!("chat login with an invalid cookie");
                            break;
                        };
                        CLIENTS.lock().unwrap().insert(a, tx.clone());
                    }
                    "NICK" => nick = rest.trim().to_string(),
                    "USER" => {
                        send(&mut wr, &format!(":lmo 001 {nick} :Welcome to LEGO Minifigures Online")).await?;
                        if let Some(a) = account {
                            for channel in DEFAULT_CHANNELS {
                                join(a, &nick, channel, &mut wr).await?;
                            }
                        }
                    }
                    "PING" => send(&mut wr, &format!(":lmo PONG lmo {rest}")).await?,
                    "JOIN" => {
                        if let Some(a) = account {
                            for channel in rest.split_whitespace().next().unwrap_or("").split(',') {
                                join(a, &nick, channel, &mut wr).await?;
                            }
                        }
                    }
                    "PART" => {
                        if let Some(a) = account {
                            for channel in rest.split_whitespace().next().unwrap_or("").split(',') {
                                part(a, channel);
                            }
                        }
                    }
                    "FILTEREDPRIVMSG" | "PRIVMSG" => {
                        let (target, text) = rest.split_once(" :").unwrap_or((rest, ""));
                        let target = target.split_whitespace().next().unwrap_or("");
                        info!(%nick, %target, %text, "chat message");
                        if let Some(a) = account {
                            let line = format!(":{nick}!u@{a}.0.0 PRIVMSG {target} :{text}");
                            relay(a, target, &line);
                            if BOT_ECHO.load(std::sync::atomic::Ordering::Relaxed) {
                                let echo = format!(":Bot!u@2823.0.0 PRIVMSG {target} :you said: {text}");
                                let _ = tx.send(echo);
                            }
                        }
                    }
                    _ => debug!(%line, "chat command ignored"),
                }
            }
            Some(out) = rx.recv() => send(&mut wr, &out).await?,
        }
    }
    if let Some(a) = account {
        let mut clients = CLIENTS.lock().unwrap();
        if clients.get(&a).is_some_and(|c| c.same_channel(&tx)) {
            clients.remove(&a);
            for members in CHANNELS.lock().unwrap().values_mut() {
                members.remove(&a);
            }
        }
    }
    Ok(())
}

async fn join(account: u32, nick: &str, channel: &str, wr: &mut tokio::net::tcp::OwnedWriteHalf) -> std::io::Result<()> {
    if !channel.starts_with('#') {
        return Ok(());
    }
    CHANNELS.lock().unwrap().entry(channel.to_ascii_lowercase()).or_default().insert(account);
    // the client treats a JOIN whose prefix nick is its own as "we joined"
    send(wr, &format!(":{nick}!u@{account}.0.0 JOIN {channel}")).await
}

fn part(account: u32, channel: &str) {
    if let Some(members) = CHANNELS.lock().unwrap().get_mut(&channel.to_ascii_lowercase()) {
        members.remove(&account);
    }
}

/// Sends `line` to the other members of `channel`.
fn relay(from: u32, channel: &str, line: &str) {
    let members: Vec<u32> = CHANNELS
        .lock()
        .unwrap()
        .get(&channel.to_ascii_lowercase())
        .map(|m| m.iter().copied().filter(|a| *a != from).collect())
        .unwrap_or_default();
    let clients = CLIENTS.lock().unwrap();
    for a in members {
        if let Some(tx) = clients.get(&a) {
            let _ = tx.send(line.to_string());
        }
    }
}

async fn send(wr: &mut tokio::net::tcp::OwnedWriteHalf, line: &str) -> std::io::Result<()> {
    debug!(%line, "->");
    wr.write_all(format!("{line}\r\n").as_bytes()).await
}

/// Shows a vicinity ("Say") line from `sender` (character instance) to `account`.
pub fn vicinity(account: u32, sender: u32, sender_name: &str, mood: i32, text: &str) {
    let name: String = sender_name.chars().filter(|c| !c.is_whitespace()).collect();
    let text = text.replace(['\r', '\n'], " ");
    let line = format!(":{name}!u@{sender}.0.0 VICINITYCHAT {name} {mood} :{text}");
    if let Some(tx) = CLIENTS.lock().unwrap().get(&account) {
        let _ = tx.send(line);
    }
}

/// Sends a line from "Server" to everyone connected to chat (control interface broadcasts);
/// the number of players reached.
pub fn broadcast(sender: u32, text: &str) -> u32 {
    let accounts: Vec<u32> = CLIENTS.lock().unwrap().keys().copied().collect();
    for &a in &accounts {
        vicinity(a, sender, "Server", 0, text);
    }
    accounts.len() as u32
}
