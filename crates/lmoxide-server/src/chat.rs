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
//!
//! The chat window only shows lines for groups it knows: channel lines (`PRIVMSG #channel`) and
//! tells (`PRIVMSG <own nick>`, the "Tell" group). `VICINITYCHAT` from a sender that isn't a
//! dynel near the player is dropped, so server lines (broadcasts, command answers) are a channel
//! message or a tell from "Server". A team is the channel `#ChatGroup_Team_D1_<team>` (the
//! window's "Team" button matches `ChatGroup_Team`): the server joins and parts its members.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{LazyLock, Mutex},
};

use futures::StreamExt;
use tokio::{
    io::AsyncWriteExt,
    net::TcpStream,
    sync::mpsc,
};
use tokio_util::codec::{FramedRead, LinesCodec};
use tracing::{Instrument, debug, info, info_span, warn};

/// character (the one the account plays) -> lines to send on its chat connection
static CLIENTS: LazyLock<Mutex<HashMap<u32, mpsc::UnboundedSender<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// character -> its IRC nick (`<name>-<PlayerID>`), the address of tells
static NICKS: LazyLock<Mutex<HashMap<u32, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// character -> its team's channel, while joined
static TEAM_CHANNELS: LazyLock<Mutex<HashMap<u32, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// channel name (lower case) -> member characters
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
/// Team channels: this prefix and the team number. Clients can't JOIN them themselves.
const TEAM_CHANNEL_PREFIX: &str = "#ChatGroup_Team_D1_";

pub async fn listen(addr: SocketAddr, max_per_ip: u32, proxies: Vec<ipnet::IpNet>) -> anyhow::Result<()> {
    let proxies = std::sync::Arc::new(proxies);
    let listener = crate::conn::bind(addr).await?;
    info!("chat listening on {addr}");
    loop {
        let (mut sock, proxy) = match listener.accept().await {
            Ok(a) => a,
            Err(e) => {
                warn!("chat: accept failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let proxies = proxies.clone();
        tokio::spawn(async move {
            // the player's address: from a trusted TCP proxy's PROXY header (proxy.rs)
            let peer = match crate::proxy::client_addr(&mut sock, proxy, &proxies).await {
                Ok(peer) => peer,
                Err(e) => {
                    warn!(%proxy, "chat: {e:#}; closed");
                    return;
                }
            };
            let Some(guard) = crate::security::connection(peer.ip(), max_per_ip) else {
                warn!(%peer, "chat: too many connections from this address, refused");
                return;
            };
            async move {
                let _guard = guard;
                match handle(sock).await {
                    Ok(()) => info!("disconnected"),
                    Err(e) => warn!("chat connection error: {e:#}"),
                }
            }
            .instrument(info_span!("chat", %peer))
            .await
        });
    }
}

async fn handle(sock: TcpStream) -> anyhow::Result<()> {
    let (rd, mut wr) = sock.into_split();
    let mut lines = FramedRead::new(rd, LinesCodec::new_with_max_length(MAX_LINE));
    let login_deadline = tokio::time::Instant::now() + LOGIN_TIMEOUT;
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    // the account logged in, and the character it plays (looked up again for every line: the
    // game login can pick it after the chat connected, `retarget`)
    let mut login = None;
    let mut account = None;
    let mut nick = String::new();
    // a player this server sent to the peer server: its chat goes there (switch.rs)
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(login_deadline), if account.is_none() => {
                anyhow::bail!("no PASS within {LOGIN_TIMEOUT:?}");
            }
            line = lines.next() => {
                let Some(line) = line.transpose()? else { break };
                debug!(%line, "<-");
                if let Some(l) = login {
                    account = Some(crate::sessions::character_of(l));
                }
                let (cmd, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
                match cmd.to_ascii_uppercase().as_str() {
                    "PASS" => {
                        let cookie: u32 = rest.trim_start_matches(':').trim().parse().unwrap_or(0);
                        let logged_in = crate::state::or_log(crate::state::db().game_session(cookie), "session", None);
                        // chat is addressed to characters: the one the account plays
                        login = logged_in;
                        account = logged_in.map(crate::sessions::character_of);
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
                            NICKS.lock().unwrap().insert(a, nick.clone());
                            for channel in DEFAULT_CHANNELS {
                                join(a, &nick, channel, &mut wr).await?;
                            }
                            set_team(a, crate::community::team_of(a));
                        }
                    }
                    "PING" => send(&mut wr, &format!(":lmo PONG lmo {rest}")).await?,
                    "JOIN" => {
                        if let Some(a) = account {
                            for channel in rest.split_whitespace().next().unwrap_or("").split(',') {
                                if is_team_channel(channel) {
                                    warn!(%nick, %channel, "chat: JOIN of a team channel refused");
                                    continue;
                                }
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
                        if let Some(a) = account
                            && crate::devcmd::is_command(text)
                        {
                            // a test command typed into a channel tab: run it in the player's
                            // game session (answers come as local chat lines), not relayed
                            let id = lmoxide_net::types::Id32::new(crate::territory::CHARACTER_TYPE, a);
                            crate::world::send_to(id, crate::world::Out::Command(text.to_string(), None));
                        } else if let Some(a) = account
                            && !target.starts_with('#')
                        {
                            tell(a, &nick, target, text);
                        } else if let Some(a) = account {
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
    if let Some(a) = login.map(crate::sessions::character_of).or(account) {
        let mut clients = CLIENTS.lock().unwrap();
        if clients.get(&a).is_some_and(|c| c.same_channel(&tx)) {
            clients.remove(&a);
            NICKS.lock().unwrap().remove(&a);
            TEAM_CHANNELS.lock().unwrap().remove(&a);
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

fn is_team_channel(channel: &str) -> bool {
    channel.to_ascii_lowercase().starts_with(&TEAM_CHANNEL_PREFIX.to_ascii_lowercase())
}

/// Puts `character` in its team's channel (`None`: out of any), joining and parting it on its
/// chat connection. Called when the team changes and when the character's chat logs in.
pub fn set_team(character: u32, team: Option<u32>) {
    let new = team.map(|t| format!("{TEAM_CHANNEL_PREFIX}{t}"));
    let old = {
        let mut teams = TEAM_CHANNELS.lock().unwrap();
        let old = match &new {
            Some(c) => teams.insert(character, c.clone()),
            None => teams.remove(&character),
        };
        if old == new {
            return;
        }
        old
    };
    let nick = NICKS.lock().unwrap().get(&character).cloned();
    let mut lines = Vec::new();
    {
        let mut channels = CHANNELS.lock().unwrap();
        if let Some(old) = &old {
            if let Some(members) = channels.get_mut(&old.to_ascii_lowercase()) {
                members.remove(&character);
                if members.is_empty() {
                    channels.remove(&old.to_ascii_lowercase());
                }
            }
            lines.extend(nick.as_ref().map(|n| format!(":{n}!u@{character}.0.0 PART {old}")));
        }
        if let Some(new) = &new {
            channels.entry(new.to_ascii_lowercase()).or_default().insert(character);
            lines.extend(nick.as_ref().map(|n| format!(":{n}!u@{character}.0.0 JOIN {new}")));
        }
    }
    if let Some(tx) = CLIENTS.lock().unwrap().get(&character) {
        for line in lines {
            let _ = tx.send(line);
        }
    }
    debug!(character, ?old, ?new, "chat: team channel");
}

/// The chat connection of `old` is `new`'s (the account now plays `new`): its channels and nick
/// move to it (the game's `rejoin` at CharInPlay then sends their JOINs). Without this, chat typed
/// after a server restart went to the account's id: test commands answered nobody and the client
/// showed no channel (the user, 2026-10-30).
pub fn retarget(old: u32, new: u32) {
    let mut clients = CLIENTS.lock().unwrap();
    if clients.contains_key(&new) {
        return;
    }
    let Some(tx) = clients.remove(&old) else { return };
    clients.insert(new, tx);
    drop(clients);
    let mut nicks = NICKS.lock().unwrap();
    if let Some(nick) = nicks.remove(&old) {
        nicks.insert(new, nick);
    }
    drop(nicks);
    let old_team = TEAM_CHANNELS.lock().unwrap().remove(&old).map(|c| c.to_ascii_lowercase());
    for (channel, members) in CHANNELS.lock().unwrap().iter_mut() {
        if members.remove(&old) && Some(channel) != old_team.as_ref() {
            members.insert(new);
        }
    }
    set_team(new, crate::community::team_of(new));
    info!(old, new, "chat: connection moved to the character played");
}

/// Sends the JOINs of a connected character's channels again (the default ones and its team's).
pub fn rejoin(character: u32) {
    let Some(nick) = NICKS.lock().unwrap().get(&character).cloned() else { return };
    let team = TEAM_CHANNELS.lock().unwrap().get(&character).cloned();
    if let Some(tx) = CLIENTS.lock().unwrap().get(&character) {
        for channel in DEFAULT_CHANNELS.iter().copied().chain(team.as_deref()) {
            let _ = tx.send(format!(":{nick}!u@{character}.0.0 JOIN {channel}"));
        }
    }
}

/// A private message ("/tell"): `target` is a nick or a character name (its name now: the nick
/// keeps the name it logged in with).
fn tell(from: u32, nick: &str, target: &str, text: &str) {
    let named = crate::sessions::character_named(target);
    let found = NICKS.lock().unwrap().iter().find_map(|(&a, n)| {
        let name = n.rsplit_once('-').map_or(n.as_str(), |(name, _)| name);
        (named == Some(a) || n.eq_ignore_ascii_case(target) || name.eq_ignore_ascii_case(target)).then(|| (a, n.clone()))
    });
    match found {
        Some((to, to_nick)) => {
            if let Some(tx) = CLIENTS.lock().unwrap().get(&to) {
                let _ = tx.send(format!(":{nick}!u@{from}.0.0 PRIVMSG {to_nick} :{text}"));
            }
        }
        None => {
            system(from, &format!("{target} is not online."));
        }
    }
}

/// A line from "Server" to one character (a tell: command answers, kick reasons).
pub fn system(character: u32, text: &str) {
    let text = text.replace(['\r', '\n'], " ");
    if let Some(c) = CAPTURES.lock().unwrap().get_mut(&character) {
        c.lines.push(text.clone());
    }
    let Some(nick) = NICKS.lock().unwrap().get(&character).cloned() else { return };
    let line = format!(":Server!u@{}.0.0 PRIVMSG {nick} :{text}", crate::game::SERVER_INSTANCE);
    if let Some(tx) = CLIENTS.lock().unwrap().get(&character) {
        let _ = tx.send(line);
    }
}

/// Server lines to a character while control interface commands run for it.
#[derive(Default)]
struct Capture {
    /// captures open (commands in flight)
    open: u32,
    lines: Vec<String>,
}

static CAPTURES: LazyLock<Mutex<HashMap<u32, Capture>>> = LazyLock::new(Default::default);

/// Starts keeping the server lines `system` sends to `character` (until `end_capture`).
pub fn capture(character: u32) {
    CAPTURES.lock().unwrap().entry(character).or_default().open += 1;
}

/// The lines kept since `capture`; with several captures open, each gets the lines so far.
pub fn end_capture(character: u32) -> Vec<String> {
    let mut captures = CAPTURES.lock().unwrap();
    let Some(c) = captures.get_mut(&character) else { return Vec::new() };
    c.open -= 1;
    if c.open == 0 { captures.remove(&character).map(|c| c.lines).unwrap_or_default() } else { c.lines.clone() }
}

/// Sends `line` to the other members of `channel`, if `from` is one of them.
fn relay(from: u32, channel: &str, line: &str) {
    let members: Vec<u32> = CHANNELS
        .lock()
        .unwrap()
        .get(&channel.to_ascii_lowercase())
        .filter(|m| m.contains(&from))
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

/// Sends a line from "Server" to everyone connected to chat (control interface broadcasts),
/// as a message in the default "Global" channel: the client doesn't show vicinity lines from a
/// sender that isn't in the playfield. The number of players reached.
pub fn broadcast(sender: u32, text: &str) -> u32 {
    let text = text.replace(['\r', '\n'], " ");
    let channel = DEFAULT_CHANNELS[0];
    let line = format!(":Server!u@{sender}.0.0 PRIVMSG {channel} :{text}");
    let clients = CLIENTS.lock().unwrap();
    for tx in clients.values() {
        let _ = tx.send(line.clone());
    }
    clients.len() as u32
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_that_connected_first_follows_the_game_login() {
        // after a server restart the chat connected as the account (9_000_001), then the game
        // logged in its character 9_000_002: the rejoin at CharInPlay reaches its connection
        let (tx, mut rx) = mpsc::unbounded_channel();
        CLIENTS.lock().unwrap().insert(9_000_001, tx);
        NICKS.lock().unwrap().insert(9_000_001, "host-0".into());
        CHANNELS.lock().unwrap().entry("#chatgroup_general_d1_1".into()).or_default().insert(9_000_001);
        crate::sessions::set_character(9_000_001, 9_000_002);
        assert!(!CLIENTS.lock().unwrap().contains_key(&9_000_001));
        assert!(CHANNELS.lock().unwrap()["#chatgroup_general_d1_1"].contains(&9_000_002));
        rejoin(9_000_002);
        assert_eq!(rx.try_recv().unwrap(), ":host-0!u@9000002.0.0 JOIN #ChatGroup_General_D1_1");
        // a later login of the same character changes nothing
        crate::sessions::set_character(9_000_001, 9_000_002);
        assert!(CLIENTS.lock().unwrap().contains_key(&9_000_002));
    }

    #[test]
    fn capture_keeps_server_lines_while_open() {
        // (no chat connection: the lines are kept all the same)
        let ch = 0xC0FFEE;
        system(ch, "before");
        capture(ch);
        system(ch, "one");
        capture(ch);
        system(ch, "two");
        assert_eq!(end_capture(ch), ["one", "two"]);
        system(ch, "three");
        assert_eq!(end_capture(ch), ["one", "two", "three"]);
        system(ch, "after");
        assert!(end_capture(ch).is_empty());
        assert!(!CAPTURES.lock().unwrap().contains_key(&ch));
    }
}
