//! Same-length, in-place patches of the client executable (`LMO.exe`, Linux `LMO`), so
//! patchers only have to copy files:
//!
//! - the login server public key: Funcom's ([`FUNCOM_SESSION_KEY`], the global `session`)
//!   replaced with the game server's (see `lmoxide_net::login`), and
//! - the chat filter URL (`http://chatfilter.funcom.com/rest/%s`, asked before every chat
//!   line) pointed at this server, exactly like `patches/patch-client.py`.
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
}
