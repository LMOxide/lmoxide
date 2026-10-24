//! Login password encryption: the client's `answer_challenge` (the Anarchy Online scheme).
//!
//! The universe agent sends `Challenge(text)`; the client answers with
//! `"<client public key>-<ciphertext>"`, both hex:
//!
//! - Diffie-Hellman over a fixed 2048-bit [`PRIME`] with generator 5: the client picks a
//!   random exponent `x`, sends `5^x mod p` and derives `secret = session^x mod p`, where
//!   `session` is the server's public key compiled into the client ([`FUNCOM_SESSION_KEY`]).
//! - The first 32 hex digits of `secret` (lowercase, no leading zeros) are a 128-bit TEA key.
//! - The plaintext `"user|challenge|game id|password|"` is prefixed with 8 random bytes and its
//!   big-endian u32 length, padded with spaces to whole 8-byte blocks (always at least one byte
//!   of padding) and TEA-encrypted in a chained mode: each block is XORed with the previous
//!   ciphertext block before encryption, unless that block's first word is 0.
//!
//! Funcom's private key is unknown, so clients are patched to carry an LMOxide server's own
//! public key in place of [`FUNCOM_SESSION_KEY`] (same length, zero-padded; see
//! [`LoginKey::public_hex`]). Found in `answer_challenge`, `encrypt` and
//! `diffie_computesharedsecret` of the Linux build; the strings are the globals `prime` and
//! `session` (identical in LMO.exe).

use num_bigint::BigUint;
use num_traits::Num;

/// The Diffie-Hellman modulus compiled into the client (global `prime`).
pub const PRIME: &str = "fd403ef9dee7e0ae2b59585dc373e9e68add16a734ee0fb88da3009d6f05efca650f5ee95a778d1896b0d5d54e62be2f15144325f7dd06c1465b3ef40480e5cab2a52ae0f9d634000e90b7a4f446826f42fb7c89e1201c6adc1bebe5efbfe93f6afb3ba6dfb657852afdd3d1c307d0b0b87120d87e3b6fa8bbe4443b8ff8ece7aa841989f2213b0081cf6b8c5c97abe148fd929efdb2589400fe32018679c6c03ec3f86e3c0e41c8b77dabdc89fa95d8b09787bd2e0d9c588a751fe1e29f5c3757e55de15484e5280a537c0477da03d3aeb434021969286e4fd6e73ef38f503e0affbf7cbeecea92f1e2e3db0fadfed255e6ad798e5566c66798777b99b1d487";

/// The generator (global `generator`).
pub const GENERATOR: u32 = 5;

/// Funcom's login server public key as shipped in the client (global `session`). Patched
/// clients carry an LMOxide server's key in its place.
pub const FUNCOM_SESSION_KEY: &str = "a738d2bc4ba9fea7a6d3e485f85f5a5c74c459940c488f8ec88ab5a326161c133bbaee9c1ee7fd0e0453b51cc8139826e0e7c1bdecc5682e2af019d862026133f4e1239d331365b3643991add017cd1039104f44e0628e0045264900a594d18d7d4914493c4c2e03e667ef9268bceae22305d0ec7dc7422346df3067569cda599013ddab68c64a16ecb796e1eb11276b3fb90800c3e5b56f34e570f8683e3b3f333acebc9d988d667726a61bbd2487d8e97bf371a783a5284eb3d2c0a768cc1f138992ebda48a81d89762216a589b7963d1df15396a3b8b7b9cbb6aa6354d2aa3a4bce05d6a5207dc7cead7567ae4d05498d97a076f4d2ffb9469ac23785cf2b";

/// `universe::LoginStatus`, the `eReason` of `UniverseInterface::AckAuthenticate` (names from
/// `universe::GetLoginStatusName`). Any non-zero value is a login problem; the client closes the
/// game for all but 10 and 38, which return to the login screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum LoginStatus {
    Ok = 0,
    NoGameServers = 5,
    ThisDimensionFull = 10,
    SystemError = 12,
    Banned = 13,
    AuthFailure = 14,
    HasAnotherOnlineCharacter = 19,
    AccountNotVerified = 24,
    MaintenanceMode = 37,
    InvalidCookie = 38,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LoginError {
    #[error("malformed answer")]
    Malformed,
    #[error("shared secret too short")]
    ShortSecret,
    #[error("decrypted answer is not valid (wrong server key?)")]
    BadPlaintext,
    #[error("invalid key")]
    BadKey,
}

/// The fields of a decrypted answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub username: String,
    pub challenge: String,
    pub game_id: u32,
    pub password: String,
}

fn prime() -> BigUint {
    BigUint::from_str_radix(PRIME, 16).unwrap()
}

/// A login server key pair. The private exponent never leaves the game server; the public key
/// is what clients are patched with.
#[derive(Clone)]
pub struct LoginKey {
    private: BigUint,
    public: BigUint,
}

impl std::fmt::Debug for LoginKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginKey").field("public", &self.public_hex()).finish_non_exhaustive()
    }
}

impl LoginKey {
    /// A new key pair with a random 256-bit private exponent.
    pub fn generate() -> Self {
        let bytes: [u8; 32] = rand::random();
        Self::from_private(BigUint::from_bytes_be(&bytes))
    }

    fn from_private(private: BigUint) -> Self {
        let public = BigUint::from(GENERATOR).modpow(&private, &prime());
        Self { private, public }
    }

    pub fn from_private_hex(hex: &str) -> Result<Self, LoginError> {
        let private = BigUint::from_str_radix(hex.trim(), 16).map_err(|_| LoginError::BadKey)?;
        if private.bits() < 64 {
            return Err(LoginError::BadKey);
        }
        Ok(Self::from_private(private))
    }

    pub fn private_hex(&self) -> String {
        self.private.to_str_radix(16)
    }

    /// The public key as patched into clients: lowercase hex, zero-padded to the length of
    /// [`FUNCOM_SESSION_KEY`] so it can replace it in place.
    pub fn public_hex(&self) -> String {
        format!("{:0>width$}", self.public.to_str_radix(16), width = FUNCOM_SESSION_KEY.len())
    }

    /// Decrypts a client's `AnswerChallenge` text.
    pub fn decrypt_answer(&self, answer: &str) -> Result<Answer, LoginError> {
        let (client_public, cipher) = answer.split_once('-').ok_or(LoginError::Malformed)?;
        let client_public = BigUint::from_str_radix(client_public, 16).map_err(|_| LoginError::Malformed)?;
        let p = prime();
        if client_public <= BigUint::from(1u32) || client_public >= p {
            return Err(LoginError::Malformed);
        }
        let secret = client_public.modpow(&self.private, &p).to_str_radix(16);
        let key = tea_key(&secret)?;
        let mut data = from_hex(cipher).ok_or(LoginError::Malformed)?;
        if data.len() < 16 || data.len() % 8 != 0 {
            return Err(LoginError::Malformed);
        }
        let mut prev: Option<[u32; 2]> = None;
        for block in data.chunks_exact_mut(8) {
            let c = [u32::from_le_bytes(block[..4].try_into().unwrap()), u32::from_le_bytes(block[4..].try_into().unwrap())];
            let mut v = tea_decrypt(c, &key);
            if let Some(p) = prev.filter(|p| p[0] != 0) {
                v = [v[0] ^ p[0], v[1] ^ p[1]];
            }
            block[..4].copy_from_slice(&v[0].to_le_bytes());
            block[4..].copy_from_slice(&v[1].to_le_bytes());
            prev = Some(c);
        }
        let len = u32::from_be_bytes(data[8..12].try_into().unwrap()) as usize;
        let text = data.get(12..12 + len).ok_or(LoginError::BadPlaintext)?;
        if data[12 + len..].iter().any(|&b| b != b' ') {
            return Err(LoginError::BadPlaintext);
        }
        let text = std::str::from_utf8(text).map_err(|_| LoginError::BadPlaintext)?;
        parse_plaintext(text)
    }
}

/// `user|challenge|game id|password|extra` (extra is empty in practice; the password may
/// contain `|`).
fn parse_plaintext(text: &str) -> Result<Answer, LoginError> {
    let mut it = text.splitn(4, '|');
    let (Some(username), Some(challenge), Some(game_id), Some(rest)) = (it.next(), it.next(), it.next(), it.next())
    else {
        return Err(LoginError::BadPlaintext);
    };
    let game_id = game_id.parse().map_err(|_| LoginError::BadPlaintext)?;
    let password = rest.rsplit_once('|').map_or(rest, |(p, _)| p);
    Ok(Answer { username: username.into(), challenge: challenge.into(), game_id, password: password.into() })
}

/// Client side of the exchange (`answer_challenge`), for tests and test clients.
pub fn answer_challenge(server_public_hex: &str, username: &str, challenge: &str, game_id: u32, password: &str) -> String {
    let p = prime();
    let x = BigUint::from_bytes_be(&rand::random::<[u8; 16]>());
    let server = BigUint::from_str_radix(server_public_hex, 16).expect("server key");
    let secret = server.modpow(&x, &p).to_str_radix(16);
    let session = BigUint::from(GENERATOR).modpow(&x, &p).to_str_radix(16);
    let key = tea_key(&secret).expect("secret");
    let text = format!("{username}|{challenge}|{game_id}|{password}|");
    let size = (text.len() + 12) / 8 * 8 + 8;
    let mut data = vec![b' '; size];
    data[..8].copy_from_slice(&rand::random::<[u8; 8]>());
    data[8..12].copy_from_slice(&(text.len() as u32).to_be_bytes());
    data[12..12 + text.len()].copy_from_slice(text.as_bytes());
    let mut prev: Option<[u32; 2]> = None;
    for block in data.chunks_exact_mut(8) {
        let mut v = [u32::from_le_bytes(block[..4].try_into().unwrap()), u32::from_le_bytes(block[4..].try_into().unwrap())];
        if let Some(p) = prev.filter(|p| p[0] != 0) {
            v = [v[0] ^ p[0], v[1] ^ p[1]];
        }
        let c = tea_encrypt(v, &key);
        block[..4].copy_from_slice(&c[0].to_le_bytes());
        block[4..].copy_from_slice(&c[1].to_le_bytes());
        prev = Some(c);
    }
    let cipher: String = data.iter().map(|b| format!("{b:02x}")).collect();
    format!("{session}-{cipher}")
}

/// The client reads 16 key bytes from the first 32 hex digits of the secret, used as four
/// little-endian words.
fn tea_key(secret_hex: &str) -> Result<[u32; 4], LoginError> {
    let bytes = from_hex(secret_hex.get(..32).ok_or(LoginError::ShortSecret)?).ok_or(LoginError::ShortSecret)?;
    Ok(std::array::from_fn(|i| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())))
}

const DELTA: u32 = 0x9e37_79b9;

fn tea_encrypt([mut v0, mut v1]: [u32; 2], k: &[u32; 4]) -> [u32; 2] {
    let mut sum = 0u32;
    for _ in 0..32 {
        sum = sum.wrapping_add(DELTA);
        v0 = v0.wrapping_add((v1 << 4).wrapping_add(k[0]) ^ v1.wrapping_add(sum) ^ (v1 >> 5).wrapping_add(k[1]));
        v1 = v1.wrapping_add((v0 << 4).wrapping_add(k[2]) ^ v0.wrapping_add(sum) ^ (v0 >> 5).wrapping_add(k[3]));
    }
    [v0, v1]
}

fn tea_decrypt([mut v0, mut v1]: [u32; 2], k: &[u32; 4]) -> [u32; 2] {
    let mut sum = DELTA.wrapping_mul(32);
    for _ in 0..32 {
        v1 = v1.wrapping_sub((v0 << 4).wrapping_add(k[2]) ^ v0.wrapping_add(sum) ^ (v0 >> 5).wrapping_add(k[3]));
        v0 = v0.wrapping_sub((v1 << 4).wrapping_add(k[0]) ^ v1.wrapping_add(sum) ^ (v1 >> 5).wrapping_add(k[1]));
        sum = sum.wrapping_sub(DELTA);
    }
    [v0, v1]
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tea_roundtrip() {
        let k = [1, 2, 3, 4];
        assert_eq!(tea_decrypt(tea_encrypt([0xdead_beef, 42], &k), &k), [0xdead_beef, 42]);
    }

    #[test]
    fn answer_roundtrip() {
        let key = LoginKey::generate();
        assert_eq!(key.public_hex().len(), FUNCOM_SESSION_KEY.len());
        for password in ["hunter2", "with|pipe", "", "a much longer password with spaces"] {
            let answer = answer_challenge(&key.public_hex(), "hosttemple", "0123abcd", 1, password);
            let got = key.decrypt_answer(&answer).unwrap();
            assert_eq!(
                got,
                Answer { username: "hosttemple".into(), challenge: "0123abcd".into(), game_id: 1, password: password.into() }
            );
        }
    }

    #[test]
    fn wrong_key_rejected() {
        let (a, b) = (LoginKey::generate(), LoginKey::generate());
        let answer = answer_challenge(&a.public_hex(), "u", "c", 1, "p");
        assert!(b.decrypt_answer(&answer).is_err());
    }

    #[test]
    fn private_key_roundtrip() {
        let key = LoginKey::generate();
        let back = LoginKey::from_private_hex(&key.private_hex()).unwrap();
        assert_eq!(back.public_hex(), key.public_hex());
    }
}
