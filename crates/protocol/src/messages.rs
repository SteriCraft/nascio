//! Control messages (reliable, ordered stream), serialized with `postcard` and framed by
//! [`crate::frame`].
//!
//! Positions of blocks are world block coordinates; the server wraps them on the torus
//! (`BlockPos::new`), so a client may send unwrapped ones.

use serde::{Deserialize, Serialize};

/// First message of the client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientHello {
    pub protocol: u32,
    /// Player name (accounts come in S7).
    pub name: String,
}

/// Answer of the server to [`ClientHello`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerHello {
    pub protocol: u32,
    pub generator_version: u32,
    /// Generation fingerprint (world hash and `[voxel]` config): client cache key.
    pub fingerprint: String,
    /// Block names; the index is the block id of the session.
    pub blocks: Vec<String>,
    /// Server UTC time, milliseconds since the Unix epoch, when the message was built.
    pub server_unix_ms: i64,
    /// Entity id of the player.
    pub player: u32,
    /// Spawn position (world metres; x and z wrapped).
    pub spawn: [f64; 3],
    /// Radius of the chunk columns the server sends, metres.
    pub chunk_radius_m: u32,
}

/// Messages from the client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello(ClientHello),
    /// Clock probe: the client's own time, echoed in [`ServerMsg::Pong`].
    Ping {
        client_ms: i64,
    },
    /// Asks to set a block (destroy: air). The server answers with the change or a refusal.
    SetBlock {
        pos: [i32; 3],
        block: u16,
    },
    /// Position and view of the player (until entity input comes in S5).
    Move {
        pos: [f64; 3],
        yaw: f32,
        pitch: f32,
    },
    /// The client already holds these columns at these versions (from its disk cache).
    Have {
        columns: Vec<([i32; 2], u32)>,
    },
    Bye,
}

/// Messages from the server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    Hello(ServerHello),
    /// Refusal of the handshake (other protocol version, server full…), then the connection
    /// closes.
    Refused {
        reason: String,
    },
    Pong {
        client_ms: i64,
        server_unix_ms: i64,
    },
    /// Blocks changed in one chunk during a tick: `(index in the chunk, block)`.
    Blocks {
        chunk: [i32; 3],
        version: u32,
        changes: Vec<(u16, u16)>,
    },
    /// A `SetBlock` that was not applied, and why (the client restores the block).
    EditRefused {
        pos: [i32; 3],
        reason: String,
    },
    /// Columns the client should drop (out of range).
    Forget {
        columns: Vec<[i32; 2]>,
    },
    Kick {
        reason: String,
    },
}

/// Serializes a message (`postcard`).
pub fn to_bytes<T: Serialize>(m: &T) -> anyhow::Result<Vec<u8>> {
    Ok(postcard::to_stdvec(m)?)
}

/// Deserializes a message; refuses trailing bytes.
pub fn from_bytes<'a, T: Deserialize<'a>>(b: &'a [u8]) -> anyhow::Result<T> {
    let (m, rest) = postcard::take_from_bytes(b)?;
    anyhow::ensure!(rest.is_empty(), "trailing bytes in a message");
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_and_stay_small() {
        let hello = ServerMsg::Hello(ServerHello {
            protocol: crate::PROTOCOL_VERSION,
            generator_version: 19,
            fingerprint: "abc".into(),
            blocks: vec!["air".into(), "granite".into()],
            server_unix_ms: 1_795_000_000_000,
            player: 7,
            spawn: [99_999.5, 64.0, 0.25],
            chunk_radius_m: 512,
        });
        let msgs = [
            hello,
            ServerMsg::Blocks {
                chunk: [3_124, -2, 0],
                version: 9,
                changes: vec![(0, 1), (32_767, 0)],
            },
            ServerMsg::Pong {
                client_ms: -5,
                server_unix_ms: 1,
            },
        ];
        for m in msgs {
            let b = to_bytes(&m).unwrap();
            assert_eq!(from_bytes::<ServerMsg>(&b).unwrap(), m);
        }
        // An edit: tag, three varint coordinates, block.
        let edit = to_bytes(&ClientMsg::SetBlock {
            pos: [99_999, 70, 5],
            block: 12,
        })
        .unwrap();
        assert!(edit.len() <= 9, "{} bytes", edit.len());
        let mut b = to_bytes(&ClientMsg::Bye).unwrap();
        b.push(0);
        assert!(from_bytes::<ClientMsg>(&b).is_err());
        assert!(from_bytes::<ClientMsg>(&[200]).is_err());
    }
}
