//! Nascio network protocol (`docs/world-spec.md` §8).
//!
//! Pure data, no networking: the chunk codec v2 and column batches ([`chunk`]), framed
//! control messages ([`messages`], [`frame`]), quantized entity states ([`entity`]), horizon
//! tiles ([`horizon`]), the sky and far ground ([`sky`]) and the clock offset estimate of the
//! client ([`clock`]). The transport (QUIC) lives in the server and the client.

pub mod chunk;
pub mod clock;
pub mod discovery;
pub mod entity;
pub mod frame;
pub mod horizon;
pub mod messages;
pub mod sky;
pub mod varint;

/// Version of the protocol, checked at the handshake. Any change of a message or a codec
/// increments it. v5: the block table (definitions, not only names) in `ServerHello`. v6: `ClientHello.auth`, a
/// secret or a token signed by the site. v7: chat (`ClientMsg::Chat`, `ServerMsg::Chat`). v8: horizon
/// tiles and `WorldSky` ([`ClientMsg::Tiles`], [`ClientMsg::Sky`]), a [`UniKind`] byte before
/// every unidirectional stream.
///
/// [`ClientMsg::Tiles`]: messages::ClientMsg::Tiles
/// [`ClientMsg::Sky`]: messages::ClientMsg::Sky
pub const PROTOCOL_VERSION: u32 = 8;

/// First byte of a unidirectional stream from the server: what follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum UniKind {
    /// A column batch ([`chunk::Column`]).
    Column = 0,
    /// A horizon tile ([`horizon::decode_tile`]).
    Tile = 1,
    /// The sky and far ground ([`sky::WorldSky`]).
    Sky = 2,
}

impl UniKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Column),
            1 => Some(Self::Tile),
            2 => Some(Self::Sky),
            _ => None,
        }
    }
}
