//! Finding servers on the local network (no networking here: the packets only).
//!
//! A client sends a [`probe`] by UDP to the discovery port (`DISCOVERY_PORT`), broadcast on
//! the local network or to one address; each server there answers with its [`ServerInfo`],
//! certificate included: the client pins it for its connection.
//!
//! The probe is padded to [`PROBE_BYTES`] and the server answers only probes of that size, so
//! the answer is never larger than the request (no amplification by a spoofed source).

use serde::{Deserialize, Serialize};

/// Default UDP port of the discovery (the game listens on 7777).
pub const DISCOVERY_PORT: u16 = 7778;
/// Size of a probe, bytes (a whole answer fits in it).
pub const PROBE_BYTES: usize = 1_200;
const PROBE_MAGIC: &[u8; 8] = b"NASCIO?\0";
const ANSWER_MAGIC: &[u8; 8] = b"NASCIO!\0";

/// What a server tells about itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub protocol: u32,
    /// UDP port of the game (QUIC).
    pub port: u16,
    /// The server's name (`server_id`).
    pub server_id: String,
    pub players: u32,
    pub max_players: u32,
    /// The server's certificate (DER), pinned by the client.
    pub cert_der: Vec<u8>,
}

/// A probe: the magic, the protocol version, zeros up to [`PROBE_BYTES`].
pub fn probe() -> Vec<u8> {
    let mut b = vec![0u8; PROBE_BYTES];
    b[..8].copy_from_slice(PROBE_MAGIC);
    b[8..12].copy_from_slice(&crate::PROTOCOL_VERSION.to_le_bytes());
    b
}

/// Whether `b` is a probe (of the full size).
pub fn is_probe(b: &[u8]) -> bool {
    b.len() == PROBE_BYTES && b.starts_with(PROBE_MAGIC)
}

/// The answer to a probe; `None` if it would not fit in [`PROBE_BYTES`].
pub fn answer(info: &ServerInfo) -> Option<Vec<u8>> {
    let mut b = ANSWER_MAGIC.to_vec();
    b.extend(postcard::to_stdvec(info).ok()?);
    (b.len() <= PROBE_BYTES).then_some(b)
}

/// Reads an answer.
pub fn read_answer(b: &[u8]) -> Option<ServerInfo> {
    let rest = b.strip_prefix(ANSWER_MAGIC)?;
    crate::messages::from_bytes(rest).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_and_answer_round_trip() {
        let p = probe();
        assert!(is_probe(&p));
        assert!(!is_probe(&p[..100]));
        let info = ServerInfo {
            protocol: crate::PROTOCOL_VERSION,
            port: 7777,
            server_id: "dev".into(),
            players: 3,
            max_players: 1_000,
            cert_der: vec![7; 400],
        };
        let a = answer(&info).unwrap();
        assert!(a.len() <= PROBE_BYTES);
        assert_eq!(read_answer(&a), Some(info));
        assert_eq!(read_answer(&p), None);
    }
}
