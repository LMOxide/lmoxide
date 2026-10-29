//! Same-length, in-place patches of the client executable (`LMO.exe`, Linux `LMO`), so
//! patchers only have to copy files:
//!
//! - the login server public key: Funcom's ([`FUNCOM_SESSION_KEY`], the global `session`)
//!   replaced with the game server's (see `lmoxide_net::login`), and
//! - the chat filter URL (`http://chatfilter.funcom.com/rest/%s`, asked before every chat
//!   line) pointed at this server, exactly like `patches/patch-client.py`, and
//! - private messages: 1.5 emptied `ChatServerInterface_c::DoTell` (used by "/tell" and the
//!   friends list), so tells went nowhere; it becomes a call of the working virtual `SendTell`
//!   (chat filter, then IRC PRIVMSG). Linux build only (see [`patch_tells`]).
//!
//! [`patch_chat_gui`] patches `Data/Gui/Default/Modules.xml`: 1.5 also showed the chat window
//! to GMs only (`stat:gmlevel!=0`), unlike live before it. [`patch_bag_open_event`] makes the
//! "You got a new minifigure" window (BagOpenEvent) load when the GUI starts.
//!
//! Both also recognise their own earlier patch, so an already patched binary can be patched
//! again for another key or host.

use lmoxide_net::login::{FUNCOM_SESSION_KEY, PRIME};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PatchError {
    #[error("login key must be {} lower-case hex characters", FUNCOM_SESSION_KEY.len())]
    BadKey,
    #[error("login key not found in the executable")]
    KeyNotFound,
    #[error("login key found more than once")]
    KeyAmbiguous,
    #[error("chat filter URL {url:?} longer than the original ({max} bytes)", url = .0, max = CHAT_FILTER_URL.len())]
    UrlTooLong(String),
    #[error("chat filter URL not found in the executable")]
    UrlNotFound,
    #[error("DoTell not found (only the Linux build is patched)")]
    TellNotFound,
    #[error("the chat window's GM condition not found")]
    ChatGuiNotFound,
    #[error("the BagOpenEvent module not found")]
    BagOpenEventNotFound,
}

/// Linux `ChatServerInterface_c::DoTell(String const&, String const&, int)` (empty: `push rbp;
/// mov rbp,rsp; pop rbp; ret`), recognised with the start of the next function (`SetCookie`).
const DO_TELL_STOCK: &[u8] = &[0x55, 0x48, 0x89, 0xe5, 0x5d, 0xc3];
/// `mov rax,[rdi]; jmp [rax+0x18]`: the vtable's SendTell(String const&, String const&) with
/// the same arguments (the int is ignored).
const DO_TELL_PATCHED: &[u8] = &[0x48, 0x8b, 0x07, 0xff, 0x60, 0x18];
const SET_COOKIE: &[u8] = &[0x55, 0x48, 0x89, 0xe5, 0x89, 0x77, 0x18, 0x5d, 0xc3];

/// The ChatGUI module criteria's GM condition, removed by [`patch_chat_gui`].
pub const CHAT_GUI_GM: &str = "stat:gmlevel!=0 &amp;&amp; !fullscreen_menu";

/// Makes "/tell" and the friends list's tells work (Linux executable). Returns the offset.
pub fn patch_tells(data: &mut [u8]) -> Result<usize, PatchError> {
    let find = |head: &[u8]| {
        let hits = memfind_all(data, &[head, SET_COOKIE].concat());
        (hits.len() == 1).then(|| hits[0])
    };
    let at = find(DO_TELL_STOCK).or_else(|| find(DO_TELL_PATCHED)).ok_or(PatchError::TellNotFound)?;
    data[at..at + DO_TELL_PATCHED.len()].copy_from_slice(DO_TELL_PATCHED);
    Ok(at)
}

/// Shows the chat window to everyone, not just GMs (`Data/Gui/Default/Modules.xml`).
pub fn patch_chat_gui(xml: &str) -> Result<String, PatchError> {
    match xml.matches(CHAT_GUI_GM).count() {
        1 => Ok(xml.replace(CHAT_GUI_GM, "!fullscreen_menu")),
        _ if xml.contains("chat_gui &amp;&amp; !fullscreen_menu") => Ok(xml.to_string()),
        _ => Err(PatchError::ChatGuiNotFound),
    }
}

/// The BagOpenEvent module's flags as shipped: loaded only by `ModuleController_c::
/// ProcessAtLoginPreloads`, which only the login screen's Login button calls
/// (`asc::ASAccountManagement::LoginAccount`). A client started with `-username`/`-password` (the
/// launcher, the client kit) never loads it, and the module's own code is what opens it (its
/// criteria `bag_open_event` is set only by BagOpenEvent.swf, on the collection's SignalItemAdded),
/// so the "Congratulations! You got a new minifigure." window never came.
const BAG_OPEN_EVENT_FLAGS: &str = r#"flags             = "GMF_PRELOAD_AT_ENTER_GAME""#;
/// GMF_PRELOAD: loaded once the GUI framework is ready (`ModuleController_c::SlotInitNewModules`).
const BAG_OPEN_EVENT_PRELOAD: &str = r#"flags             = "GMF_PRELOAD""#;

/// Loads the BagOpenEvent module however the client logged in (`Data/Gui/Default/Modules.xml`).
pub fn patch_bag_open_event(xml: &str) -> Result<String, PatchError> {
    let start = xml.find(r#"<Module name="BagOpenEvent""#).ok_or(PatchError::BagOpenEventNotFound)?;
    let end = start + xml[start..].find("/>").ok_or(PatchError::BagOpenEventNotFound)?;
    let module = &xml[start..end];
    if module.contains(BAG_OPEN_EVENT_PRELOAD) {
        return Ok(xml.to_string());
    }
    if !module.contains(BAG_OPEN_EVENT_FLAGS) {
        return Err(PatchError::BagOpenEventNotFound);
    }
    Ok(format!("{}{}{}", &xml[..start], module.replace(BAG_OPEN_EVENT_FLAGS, BAG_OPEN_EVENT_PRELOAD), &xml[end..]))
}

/// The chat filter format string as shipped.
pub const CHAT_FILTER_URL: &[u8] = b"http://chatfilter.funcom.com/rest/%s";

/// How far from the global `prime` the `session` key can be (it sits right after it in
/// LMO.exe and right before it in the Linux build).
const SESSION_SEARCH_WINDOW: usize = 2048;

fn is_hex_key(s: &[u8]) -> bool {
    s.len() == FUNCOM_SESSION_KEY.len() && s.iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

/// Offset of the login public key: Funcom's key if present (must be unique), otherwise the one
/// 512-hex-digit string next to the Diffie-Hellman prime (a key patched in earlier).
pub fn find_session_key(data: &[u8]) -> Result<usize, PatchError> {
    let key = FUNCOM_SESSION_KEY.as_bytes();
    let mut hits = memfind_all(data, key);
    match hits.len() {
        1 => return Ok(hits[0]),
        0 => {}
        _ => return Err(PatchError::KeyAmbiguous),
    }
    let prime = memfind_all(data, PRIME.as_bytes());
    let &[p] = prime.as_slice() else { return Err(PatchError::KeyNotFound) };
    let (lo, hi) = (p.saturating_sub(SESSION_SEARCH_WINDOW), (p + PRIME.len() + SESSION_SEARCH_WINDOW).min(data.len()));
    // Maximal runs of lower-case hex digits of exactly the key length, other than the prime.
    let is_hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    let mut i = lo;
    while i < hi {
        if !is_hex(data[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < data.len() && is_hex(data[i]) {
            i += 1;
        }
        if i - start == key.len() && start != p && (start == 0 || !is_hex(data[start - 1])) {
            hits.push(start);
        }
    }
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(PatchError::KeyNotFound),
        _ => Err(PatchError::KeyAmbiguous),
    }
}

/// Replaces the login public key with `new_key` (512 hex characters, as written by the game
/// server). Returns the offset.
pub fn patch_login_key(data: &mut [u8], new_key: &str) -> Result<usize, PatchError> {
    if !is_hex_key(new_key.as_bytes()) {
        return Err(PatchError::BadKey);
    }
    let at = find_session_key(data)?;
    data[at..at + new_key.len()].copy_from_slice(new_key.as_bytes());
    Ok(at)
}

/// Points the chat filter at `http://<host>/rest/%s` (`host` = `host:port`). Returns the offset.
pub fn patch_chat_filter(data: &mut [u8], host: &str) -> Result<usize, PatchError> {
    let url = format!("http://{host}/rest/%s");
    if url.len() > CHAT_FILTER_URL.len() {
        return Err(PatchError::UrlTooLong(url));
    }
    let at = match memfind_all(data, CHAT_FILTER_URL).first() {
        Some(&at) => at,
        None => {
            // Already patched: find our earlier URL by its suffix (as patch-client.py does).
            let suffix = b"/rest/%s\0";
            let end = memfind_all(data, suffix)
                .into_iter()
                .find(|&e| {
                    let start = string_start(data, e);
                    data[start..].starts_with(b"http://") && e + suffix.len() - start <= CHAT_FILTER_URL.len() + 1
                })
                .ok_or(PatchError::UrlNotFound)?;
            string_start(data, end)
        }
    };
    let slot = &mut data[at..at + CHAT_FILTER_URL.len() + 1];
    slot.fill(0);
    slot[..url.len()].copy_from_slice(url.as_bytes());
    Ok(at)
}

/// Start of the NUL-terminated string containing `at`.
fn string_start(data: &[u8], mut at: usize) -> usize {
    while at > 0 && data[at - 1] != 0 {
        at -= 1;
    }
    at
}

fn memfind_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() || hay.len() < needle.len() {
        return out;
    }
    let first = needle[0];
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        match hay[i..hay.len() - needle.len() + 1].iter().position(|&b| b == first) {
            Some(off) => {
                i += off;
                if &hay[i..i + needle.len()] == needle {
                    out.push(i);
                    i += needle.len();
                } else {
                    i += 1;
                }
            }
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake executable laid out like LMO.exe: prime, padding, session key, chat URL.
    fn fake_exe(prime_first: bool) -> Vec<u8> {
        let mut d = b"\x7fELF junk %s\0\0".to_vec();
        let (a, b) = if prime_first { (PRIME, FUNCOM_SESSION_KEY) } else { (FUNCOM_SESSION_KEY, PRIME) };
        d.extend_from_slice(a.as_bytes());
        d.extend_from_slice(&[0; 24]);
        d.extend_from_slice(b.as_bytes());
        d.extend_from_slice(b"\0\0\0\0%.2X\0more\0");
        d.extend_from_slice(CHAT_FILTER_URL);
        d.extend_from_slice(b"\0tail");
        d
    }

    fn count(hay: &[u8], needle: &[u8]) -> usize {
        memfind_all(hay, needle).len()
    }

    #[test]
    fn login_key_patch_and_repatch() {
        let k1 = format!("{:0>512}", "1234abcd");
        let k2 = "f".repeat(512);
        for prime_first in [true, false] {
            let orig = fake_exe(prime_first);
            let mut d = orig.clone();
            let at = patch_login_key(&mut d, &k1).unwrap();
            assert_eq!(d.len(), orig.len());
            assert_eq!(count(&d, k1.as_bytes()), 1);
            assert_eq!(count(&d, FUNCOM_SESSION_KEY.as_bytes()), 0);
            assert_eq!(count(&d, PRIME.as_bytes()), 1);
            // found again next to the prime and replaced
            assert_eq!(patch_login_key(&mut d, &k2).unwrap(), at);
            assert_eq!(count(&d, k2.as_bytes()), 1);
            assert_eq!(count(&d, k1.as_bytes()), 0);
        }
    }

    #[test]
    fn login_key_errors() {
        let mut d = fake_exe(true);
        assert_eq!(patch_login_key(&mut d, "abc"), Err(PatchError::BadKey));
        assert_eq!(patch_login_key(&mut d, &"G".repeat(512)), Err(PatchError::BadKey));
        let mut none = b"nothing here".to_vec();
        assert_eq!(patch_login_key(&mut none, &"a".repeat(512)), Err(PatchError::KeyNotFound));
        let mut twice = fake_exe(true);
        twice.extend_from_slice(FUNCOM_SESSION_KEY.as_bytes());
        assert_eq!(patch_login_key(&mut twice, &"a".repeat(512)), Err(PatchError::KeyAmbiguous));
    }

    #[test]
    fn chat_filter_like_patch_client_py() {
        let orig = fake_exe(true);
        let mut d = orig.clone();
        let at = patch_chat_filter(&mut d, "127.0.0.1:8080").unwrap();
        assert_eq!(d.len(), orig.len());
        let mut expected = b"http://127.0.0.1:8080/rest/%s".to_vec();
        expected.resize(CHAT_FILTER_URL.len() + 1, 0);
        assert_eq!(&d[at..at + expected.len()], &expected[..]);
        assert_eq!(&d[at + expected.len()..at + expected.len() + 4], b"tail");
        // re-patch to another host
        assert_eq!(patch_chat_filter(&mut d, "192.168.100.100:18080").unwrap(), at);
        assert!(count(&d, b"http://192.168.100.100:18080/rest/%s\0") == 1);
        assert!(count(&d, b"127.0.0.1") == 0);
        assert!(matches!(patch_chat_filter(&mut d, "a-very-long-host-name.example.org:8080"), Err(PatchError::UrlTooLong(_))));
        assert_eq!(patch_chat_filter(&mut b"no url".to_vec(), "h:1"), Err(PatchError::UrlNotFound));
    }

    #[test]
    fn tells_patched_once_and_again() {
        let mut d = [b"\x90\x90".as_slice(), DO_TELL_STOCK, SET_COOKIE, b"\xc3"].concat();
        let at = patch_tells(&mut d).unwrap();
        assert_eq!(&d[at..at + 6], DO_TELL_PATCHED);
        assert_eq!(patch_tells(&mut d), Ok(at));
        // an empty function elsewhere (no SetCookie after it) is left alone
        assert_eq!(patch_tells(&mut [DO_TELL_STOCK, b"\x90"].concat()), Err(PatchError::TellNotFound));
    }

    #[test]
    fn chat_gui_condition_removed() {
        let xml = r#"criteria = "chat_gui &amp;&amp; stat:gmlevel!=0 &amp;&amp; !fullscreen_menu &amp;&amp; x""#;
        let patched = patch_chat_gui(xml).unwrap();
        assert_eq!(patched, r#"criteria = "chat_gui &amp;&amp; !fullscreen_menu &amp;&amp; x""#);
        assert_eq!(patch_chat_gui(&patched).unwrap(), patched);
        assert_eq!(patch_chat_gui("other"), Err(PatchError::ChatGuiNotFound));
    }

    #[test]
    fn bag_open_event_preloaded() {
        let xml = concat!(
            "<Module name=\"Minimap\"\n flags             = \"GMF_PRELOAD_AT_ENTER_GAME\"\n/>\n",
            "<Module name=\"BagOpenEvent\"\n flags             = \"GMF_PRELOAD_AT_ENTER_GAME\"\n criteria = \"bag_open_event\"\n/>"
        );
        let patched = patch_bag_open_event(xml).unwrap();
        assert_eq!(patched.matches("\"GMF_PRELOAD\"").count(), 1);
        assert!(patched.starts_with("<Module name=\"Minimap\"\n flags             = \"GMF_PRELOAD_AT_ENTER_GAME\""));
        assert_eq!(patch_bag_open_event(&patched).unwrap(), patched);
        assert_eq!(patch_bag_open_event("other"), Err(PatchError::BagOpenEventNotFound));
    }
}
