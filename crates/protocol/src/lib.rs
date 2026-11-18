//! Nascio network protocol (proposed spec §7, `docs/reports/spec-proposee.md`).
//!
//! Pure data, no networking: the chunk codec v2 and column batches ([`chunk`]), framed
//! control messages ([`messages`], [`frame`]), quantized entity states ([`entity`]) and the
//! clock offset estimate of the client ([`clock`]). The transport (QUIC) lives in the server
//! and the client.

pub mod chunk;
pub mod clock;
pub mod entity;
pub mod frame;
pub mod messages;
pub mod varint;

/// Version of the protocol, checked at the handshake. Any change of a message or a codec
/// increments it. v5: the block table (definitions, not only names) in `ServerHello`.
pub const PROTOCOL_VERSION: u32 = 5;
