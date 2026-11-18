//! Login tokens issued by the site (chantier 2): the site, which holds the Ed25519 private
//! key, signs who may play on which server; the server only knows the public key.
//!
//! A token is `base64url(payload ‖ signature)`, without padding: the payload is the postcard
//! encoding of [`Claims`], the signature (64 bytes, Ed25519) covers the payload. The server
//! checks, in order: the signature, the version, the audience (its `server_id`), the name,
//! the dates (with [`SKEW_MS`] of clock tolerance) and that the nonce was never seen.
//!
//! Nonces are kept in memory until their token expires. A restart forgets them, so a token
//! that may have been spent before the start is refused: one issued before `start + SKEW_MS`
//! (a token used just before the start, by a site whose clock runs `SKEW_MS` ahead, bears up
//! to that time). With synchronised clocks, the site's tokens are therefore refused during
//! the first `SKEW_MS` after a start; the client asks for a new one.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Context, bail, ensure};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};

/// Format of the claims.
pub const TOKEN_VERSION: u8 = 1;
/// Clock difference tolerated between the site and the server, ms.
pub const SKEW_MS: i64 = 30_000;
/// Longest validity accepted (the site issues tokens for 5 minutes), ms.
pub const MAX_TTL_MS: i64 = 5 * 60_000;
/// Ed25519 signature length.
const SIG: usize = 64;
/// Longest token read (a name of 16 bytes and a server id of 64 make about 200 bytes).
pub const MAX_TOKEN_LEN: usize = 512;

/// What the site vouches for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub version: u8,
    /// Account name; the account is created at its first login.
    pub name: String,
    /// The server the token is for (its `server_id`).
    pub server: String,
    /// Issued and expiry times, ms since the Unix epoch (UTC).
    pub issued_unix_ms: i64,
    pub expires_unix_ms: i64,
    /// Random; a token is used once.
    pub nonce: [u8; 16],
}

/// Why a token is refused (told to the client: it reveals nothing secret).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenError {
    Malformed,
    BadSignature,
    Version(u8),
    WrongServer,
    BadName,
    Expired,
    NotYetValid,
    TooLong,
    BeforeStart,
    Replayed,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => write!(f, "malformed token"),
            Self::BadSignature => write!(f, "token not signed by the site"),
            Self::Version(v) => write!(f, "token version {v} (server: {TOKEN_VERSION})"),
            Self::WrongServer => write!(f, "token issued for another server"),
            Self::BadName => write!(f, "token for an invalid name"),
            Self::Expired => write!(f, "token expired"),
            Self::NotYetValid => write!(f, "token issued in the future (check the clock)"),
            Self::TooLong => write!(f, "token valid for too long"),
            Self::BeforeStart => write!(
                f,
                "token issued before the server started: ask for a new one in {} s",
                SKEW_MS / 1_000
            ),
            Self::Replayed => write!(f, "token already used"),
        }
    }
}

impl std::error::Error for TokenError {}

/// URL-safe base64 without padding (RFC 4648 §5).
pub fn b64_encode(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..=c.len() {
            out.push(A[(n >> (18 - 6 * k) & 63) as usize] as char);
        }
    }
    out
}

/// Inverse of [`b64_encode`]; `None` on a character outside the alphabet or a bad length.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32)
    }
    let s = s.as_bytes();
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for c in s.chunks(4) {
        let mut n = 0u32;
        for (k, &b) in c.iter().enumerate() {
            n |= val(b)? << (18 - 6 * k);
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..c.len()]);
        // Canonical form only: the unused low bits are zero.
        if c.len() < 4 && n & ((1 << (24 - 8 * (c.len() - 1))) - 1) != 0 {
            return None;
        }
    }
    Some(out)
}

/// A new site key: PKCS#8 bytes (keep them secret) and the public key.
pub fn generate_key() -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| anyhow::anyhow!("key generation failed"))?;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .map_err(|_| anyhow::anyhow!("bad generated key"))?;
    Ok((pkcs8.as_ref().to_vec(), pair.public_key().as_ref().to_vec()))
}

/// The site's signing key.
pub struct SiteKey(Ed25519KeyPair);

impl SiteKey {
    pub fn from_pkcs8(pkcs8: &[u8]) -> anyhow::Result<Self> {
        Ed25519KeyPair::from_pkcs8(pkcs8)
            .map(Self)
            .map_err(|_| anyhow::anyhow!("not an Ed25519 PKCS#8 key"))
    }

    /// From the 32-byte Ed25519 seed (RFC 8032 private key); the test vectors use it.
    pub fn from_seed(seed: &[u8; 32]) -> anyhow::Result<Self> {
        Ed25519KeyPair::from_seed_unchecked(seed)
            .map(Self)
            .map_err(|_| anyhow::anyhow!("bad Ed25519 seed"))
    }

    /// Reads a key written by `site keygen` (base64url text).
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_pkcs8(&b64_decode(text.trim()).context("the key is not base64url")?)
    }

    pub fn public_key(&self) -> Vec<u8> {
        self.0.public_key().as_ref().to_vec()
    }

    /// A token for `name` on `server`, valid `ttl_ms` from `now`.
    pub fn issue(
        &self,
        name: &str,
        server: &str,
        now_unix_ms: i64,
        ttl_ms: i64,
    ) -> anyhow::Result<String> {
        let mut nonce = [0u8; 16];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| anyhow::anyhow!("no system randomness"))?;
        self.sign(&Claims {
            version: TOKEN_VERSION,
            name: name.to_owned(),
            server: server.to_owned(),
            issued_unix_ms: now_unix_ms,
            expires_unix_ms: now_unix_ms + ttl_ms,
            nonce,
        })
    }

    /// Signs any claims (tests forge bad ones with it).
    pub fn sign(&self, claims: &Claims) -> anyhow::Result<String> {
        let mut bytes = postcard::to_stdvec(claims)?;
        let sig = self.0.sign(&bytes);
        bytes.extend_from_slice(sig.as_ref());
        ensure!(b64_len(bytes.len()) <= MAX_TOKEN_LEN, "token too long");
        Ok(b64_encode(&bytes))
    }
}

fn b64_len(n: usize) -> usize {
    (n * 4).div_ceil(3)
}

/// Checks tokens for one server.
pub struct Verifier {
    key: UnparsedPublicKey<Vec<u8>>,
    server_id: String,
    started_unix_ms: i64,
    /// Nonces seen, with the time after which their token can no longer be accepted.
    seen: Mutex<HashMap<[u8; 16], i64>>,
}

impl Verifier {
    /// `public_key`: the site's (32 bytes); `started_unix_ms`: when the server started.
    pub fn new(public_key: &[u8], server_id: &str, started_unix_ms: i64) -> anyhow::Result<Self> {
        if public_key.len() != 32 {
            bail!(
                "an Ed25519 public key has 32 bytes, not {}",
                public_key.len()
            );
        }
        ensure!(!server_id.is_empty(), "server_id is empty");
        Ok(Self {
            key: UnparsedPublicKey::new(&ED25519, public_key.to_vec()),
            server_id: server_id.to_owned(),
            started_unix_ms,
            seen: Mutex::new(HashMap::new()),
        })
    }

    /// Checks a token at `now`; a valid one is spent (its nonce is kept).
    pub fn verify(&self, token: &str, now_unix_ms: i64) -> Result<Claims, TokenError> {
        if token.len() > MAX_TOKEN_LEN {
            return Err(TokenError::Malformed);
        }
        let bytes = b64_decode(token).ok_or(TokenError::Malformed)?;
        if bytes.len() <= SIG {
            return Err(TokenError::Malformed);
        }
        let (payload, sig) = bytes.split_at(bytes.len() - SIG);
        self.key
            .verify(payload, sig)
            .map_err(|_| TokenError::BadSignature)?;
        // Signed by the site: what follows is the site's word, still checked.
        let (claims, rest) =
            postcard::take_from_bytes::<Claims>(payload).map_err(|_| TokenError::Malformed)?;
        if !rest.is_empty() {
            return Err(TokenError::Malformed);
        }
        if claims.version != TOKEN_VERSION {
            return Err(TokenError::Version(claims.version));
        }
        if claims.server != self.server_id {
            return Err(TokenError::WrongServer);
        }
        if crate::accounts::check_name(&claims.name).is_err() {
            return Err(TokenError::BadName);
        }
        if claims.expires_unix_ms - claims.issued_unix_ms > MAX_TTL_MS {
            return Err(TokenError::TooLong);
        }
        if claims.issued_unix_ms > now_unix_ms + SKEW_MS {
            return Err(TokenError::NotYetValid);
        }
        if now_unix_ms > claims.expires_unix_ms + SKEW_MS {
            return Err(TokenError::Expired);
        }
        if claims.issued_unix_ms < self.started_unix_ms + SKEW_MS {
            return Err(TokenError::BeforeStart);
        }
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        seen.retain(|_, until| *until >= now_unix_ms);
        if seen.contains_key(&claims.nonce) {
            return Err(TokenError::Replayed);
        }
        seen.insert(claims.nonce, claims.expires_unix_ms + SKEW_MS);
        Ok(claims)
    }

    /// Nonces kept (statistics, tests).
    pub fn nonces(&self) -> usize {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_800_000_000_000;

    fn setup() -> (SiteKey, Verifier) {
        let (pkcs8, public) = generate_key().unwrap();
        let site = SiteKey::from_pkcs8(&pkcs8).unwrap();
        assert_eq!(site.public_key(), public);
        let v = Verifier::new(&public, "eu-1", T0).unwrap();
        (site, v)
    }

    fn claims(name: &str, issued: i64, ttl: i64) -> Claims {
        Claims {
            version: TOKEN_VERSION,
            name: name.into(),
            server: "eu-1".into(),
            issued_unix_ms: issued,
            expires_unix_ms: issued + ttl,
            nonce: [7; 16],
        }
    }

    #[test]
    fn base64url_round_trips_and_refuses_junk() {
        for n in 0..70 {
            let b: Vec<u8> = (0..n).map(|k| (k * 37 + n) as u8).collect();
            let s = b64_encode(&b);
            assert!(!s.contains(['=', '+', '/']));
            assert_eq!(b64_decode(&s).unwrap(), b, "{n}");
        }
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64_encode(&[0xfb, 0xff]), "-_8");
        for bad in ["A", "Zm9v=", "Zm+v", "Zm9vY", "Zh"] {
            assert!(b64_decode(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_valid_token_is_accepted_once() {
        let (site, v) = setup();
        let t0 = T0 + SKEW_MS;
        let t = site.issue("alice", "eu-1", t0 + 1_000, MAX_TTL_MS).unwrap();
        assert!(t.len() < 200, "{}", t.len());
        let c = v.verify(&t, t0 + 2_000).unwrap();
        assert_eq!(c.name, "alice");
        assert_eq!(v.verify(&t, t0 + 3_000), Err(TokenError::Replayed));
        // Another token for the same name is fine.
        let t2 = site.issue("alice", "eu-1", t0 + 4_000, MAX_TTL_MS).unwrap();
        assert!(v.verify(&t2, t0 + 5_000).is_ok());
        assert_eq!(v.nonces(), 2);
        // Nonces of expired tokens are forgotten.
        let late = t0 + 4_000 + MAX_TTL_MS + SKEW_MS + 1;
        let t3 = site.issue("bob", "eu-1", late, 60_000).unwrap();
        assert!(v.verify(&t3, late).is_ok());
        assert_eq!(v.nonces(), 1);
    }

    #[test]
    fn bad_tokens_are_refused_for_the_right_reason() {
        let (site, v) = setup();
        let now = T0 + 10 * 60_000;
        // Another site key: bad signature.
        let (other, _) = generate_key().unwrap();
        let other = SiteKey::from_pkcs8(&other).unwrap();
        let t = other.issue("alice", "eu-1", now, 60_000).unwrap();
        assert_eq!(v.verify(&t, now), Err(TokenError::BadSignature));
        // One flipped byte of the payload: bad signature.
        let mut bytes = b64_decode(&site.issue("alice", "eu-1", now, 60_000).unwrap()).unwrap();
        bytes[3] ^= 1;
        assert_eq!(
            v.verify(&b64_encode(&bytes), now),
            Err(TokenError::BadSignature)
        );
        assert_eq!(v.verify("not a token!", now), Err(TokenError::Malformed));
        assert_eq!(v.verify("", now), Err(TokenError::Malformed));
        assert_eq!(
            v.verify(&"A".repeat(MAX_TOKEN_LEN + 4), now),
            Err(TokenError::Malformed)
        );
        let sign = |c: &Claims| site.sign(c).unwrap();
        let mut c = claims("alice", now, 60_000);
        c.version = 2;
        assert_eq!(v.verify(&sign(&c), now), Err(TokenError::Version(2)));
        let mut c = claims("alice", now, 60_000);
        c.server = "us-1".into();
        assert_eq!(v.verify(&sign(&c), now), Err(TokenError::WrongServer));
        let c = claims("bad name", now, 60_000);
        assert_eq!(v.verify(&sign(&c), now), Err(TokenError::BadName));
        let c = claims("alice", now, MAX_TTL_MS + 1);
        assert_eq!(v.verify(&sign(&c), now), Err(TokenError::TooLong));
        // Dates, at the edges of the tolerance.
        let c = claims("alice", now - 60_000 - SKEW_MS - 1, 60_000);
        assert_eq!(v.verify(&sign(&c), now), Err(TokenError::Expired));
        let c = claims("alice", now + SKEW_MS + 1, 60_000);
        assert_eq!(v.verify(&sign(&c), now), Err(TokenError::NotYetValid));
        let mut c = claims("alice", now + SKEW_MS, 60_000);
        c.nonce = [1; 16];
        assert!(v.verify(&sign(&c), now).is_ok(), "within the tolerance");
        let mut c = claims("alice", now - 60_000 - SKEW_MS, 60_000);
        c.nonce = [2; 16];
        assert!(v.verify(&sign(&c), now).is_ok(), "within the tolerance");
    }

    #[test]
    fn tokens_issued_before_the_start_are_refused() {
        let (site, v) = setup();
        // Spent just before a restart by a site SKEW_MS ahead, it bears up to T0 + SKEW_MS:
        // refused, though fresh and within the tolerance.
        for issued in [T0 - 60_000, T0, T0 + SKEW_MS - 1] {
            let t = site.issue("alice", "eu-1", issued, 60_000).unwrap();
            assert_eq!(v.verify(&t, T0 + 1_000), Err(TokenError::BeforeStart));
        }
        // From then on: accepted (here, at once, from a site SKEW_MS ahead).
        let t = site.issue("alice", "eu-1", T0 + SKEW_MS, 60_000).unwrap();
        assert!(v.verify(&t, T0).is_ok());
    }

    /// The vectors of `spec-proposee.md` §7.2: a site written in another language checks its
    /// implementation against them.
    #[test]
    fn fixed_vectors() {
        const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
        const PUBLIC: &str = "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8";
        const PUBLIC_B64: &str = "A6EHv_POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg";
        const PAYLOAD: &str = concat!(
            "0105616c6963650465752d3180c0e285e368c08f8786e368",
            "a0a1a2a3a4a5a6a7a8a9aaabacadaeaf"
        );
        const SIGNATURE: &str = concat!(
            "dcc27d3b4a7db6312a01d13fef72eb799d6d31bd823e03e11cdbdaa09de5ed04",
            "7dd3a820d744721b2871441d61af9c7f0919e3f26684d7fc9ba5f53eab981808"
        );
        const TOKEN: &str = concat!(
            "AQVhbGljZQRldS0xgMDiheNowI-HhuNooKGio6SlpqeoqaqrrK2ur9zCfTtKfbYxKgHR",
            "P-9y63mdbTG9gj4D4Rzb2qCd5e0EfdOoINdEchsocUQdYa-cfwkZ4_JmhNf8m6X1PquYGAg"
        );
        let unhex = |h: &str| -> Vec<u8> {
            (0..h.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
                .collect()
        };
        let site = SiteKey::from_seed(&unhex(SEED).try_into().unwrap()).unwrap();
        assert_eq!(site.public_key(), unhex(PUBLIC));
        assert_eq!(b64_encode(&site.public_key()), PUBLIC_B64);
        let claims = Claims {
            version: TOKEN_VERSION,
            name: "alice".into(),
            server: "eu-1".into(),
            issued_unix_ms: 1_800_000_000_000,
            expires_unix_ms: 1_800_000_300_000,
            nonce: std::array::from_fn(|k| (0xa0 + k) as u8),
        };
        // The payload byte by byte, as the spec describes it (independent of postcard).
        fn varint(mut v: u64, out: &mut Vec<u8>) {
            while v >= 0x80 {
                out.push((v as u8 & 0x7f) | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
        }
        let zigzag = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
        let mut by_hand = vec![claims.version];
        for text in [&claims.name, &claims.server] {
            varint(text.len() as u64, &mut by_hand);
            by_hand.extend_from_slice(text.as_bytes());
        }
        varint(zigzag(claims.issued_unix_ms), &mut by_hand);
        varint(zigzag(claims.expires_unix_ms), &mut by_hand);
        by_hand.extend_from_slice(&claims.nonce);
        assert_eq!(by_hand, unhex(PAYLOAD));
        assert_eq!(postcard::to_stdvec(&claims).unwrap(), by_hand);
        // Ed25519 is deterministic: the token is fixed.
        let token = site.sign(&claims).unwrap();
        assert_eq!(token, TOKEN);
        let raw = b64_decode(TOKEN).unwrap();
        assert_eq!(raw[..by_hand.len()], by_hand[..]);
        assert_eq!(raw[by_hand.len()..], unhex(SIGNATURE)[..]);
        // And the server accepts it (for "eu-1", started long before).
        let v = Verifier::new(&unhex(PUBLIC), "eu-1", 1_700_000_000_000).unwrap();
        assert_eq!(v.verify(TOKEN, 1_800_000_001_000).unwrap(), claims);
    }

    #[test]
    fn keys_are_checked() {
        assert!(Verifier::new(&[0; 31], "eu-1", 0).is_err());
        assert!(Verifier::new(&[0; 32], "", 0).is_err());
        assert!(SiteKey::from_pkcs8(&[1, 2, 3]).is_err());
    }
}
