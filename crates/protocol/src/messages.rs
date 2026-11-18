//! Control messages (reliable, ordered stream), serialized with `postcard` and framed by
//! [`crate::frame`]. The player's movement goes in datagrams ([`crate::player`]).
//!
//! Positions of blocks are world block coordinates; the server wraps them on the torus
//! (`BlockPos::new`), so a client may send unwrapped ones.

use nascio_chunk::BlockDef;
use nascio_physics::{Mode, PlayerParams};
use serde::{Deserialize, Serialize};

/// First message of the client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientHello {
    pub protocol: u32,
    /// Account name.
    pub name: String,
    pub auth: Auth,
}

/// How a client proves who it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Auth {
    /// The account's secret (the connection is encrypted; the server keeps only a hash).
    /// Servers accept it from administrators only, unless configured otherwise.
    Secret(String),
    /// A token from the site: base64url, valid a few minutes, used once; it names the
    /// account (which must be `name`) and the server.
    Token(String),
}

/// Answer of the server to [`ClientHello`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerHello {
    pub protocol: u32,
    pub generator_version: u32,
    /// Generation fingerprint (world hash and `[voxel]` config): client cache key.
    pub fingerprint: String,
    /// The block table of the world (names, colours, matter); the index is the block id of
    /// the session. The client meshes with it and reads no block file of its own.
    pub blocks: Vec<BlockDef>,
    /// Server UTC time, milliseconds since the Unix epoch, when the message was built.
    pub server_unix_ms: i64,
    /// Entity id of the player.
    pub player: u32,
    /// Spawn position (world metres; x and z wrapped).
    pub spawn: [f64; 3],
    /// Radius of the chunk columns the server sends, metres.
    pub chunk_radius_m: u32,
    /// The physics of the players: the client predicts its own with these values.
    pub physics: PlayerParams,
    /// How the player moves at first.
    pub mode: Mode,
    /// The account is an administrator at login: it may ask to fly (`ClientMsg::Fly`).
    pub admin: bool,
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
    ///
    /// `cmd` is the number of the newest movement command sent when the player clicked
    /// (`None`: none sent yet): the server measures the reach from the eyes as they were
    /// after that command, whatever the latency.
    SetBlock {
        pos: [i32; 3],
        block: u16,
        cmd: Option<u32>,
    },
    /// Entity datagrams received (their `seq`): their states can serve as baselines.
    Ack {
        seqs: Vec<u32>,
    },
    /// The client already holds these columns at these versions (from its disk cache).
    Have {
        columns: Vec<([i32; 2], u64)>,
    },
    /// A chat line for every player (at most [`MAX_CHAT_CHARS`] characters).
    Chat {
        text: String,
    },
    /// Asks horizon tiles (at most [`crate::horizon::MAX_TILES_PER_REQUEST`]); each comes on
    /// a unidirectional stream ([`crate::UniKind::Tile`]), after the columns near the player.
    Tiles {
        keys: Vec<nascio_chunk::horizon::TileKey>,
    },
    /// Asks the sky and far ground ([`crate::sky::WorldSky`]), once per connection; the client
    /// skips it when its disk cache holds them for the world's fingerprint.
    Sky,
    Bye,
    /// Back to life after [`ServerMsg::Died`], at the account's spawn point.
    Respawn,
    /// Asks to fly or to walk again; administrators only (others are refused and logged).
    Fly(bool),
    /// Asks a column again (an edit fell below its chunks: the client forgot it). Ignored for
    /// a column the server is not sending to the client.
    Resend {
        column: [i32; 2],
    },
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
    ///
    /// `lsn` is the number of the last of them. A client applies them all to a column of
    /// lower version (values are absolute: replaying older ones of the batch is harmless),
    /// keeps them for a column still on its way, and then sets the column's version to `lsn`.
    Blocks {
        chunk: [i32; 3],
        lsn: u64,
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
    /// A line of the chat.
    Chat(Chat),
    /// The player died: it no longer moves until it asks [`ClientMsg::Respawn`].
    Died {
        cause: DeathCause,
    },
    /// The player is alive again at `pos` (its state follows in a `PlayerState`).
    Respawned {
        pos: [f64; 3],
    },
}

/// Why a player died; the client words it in its own language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeathCause {
    /// Shut in blocks under a solid block (an edit or a fall of blocks on it).
    Suffocation,
    /// Touched the mantle's magma.
    Magma,
}

/// Longest chat line, characters (longer ones are cut by the server).
pub const MAX_CHAT_CHARS: usize = 256;

/// A line of the chat; the client words it in its own language.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Chat {
    /// A player logged in.
    Joined(String),
    /// A player left (not after a ban, which has its own line).
    Left(String),
    /// A player was banned.
    Banned(String),
    /// A player wrote to everyone.
    Say { from: String, text: String },
    /// An administrator wrote, to everyone or to this player only (`[Admin]: <text>`).
    Admin(String),
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
            blocks: nascio_chunk::BlockRegistry::builtin().blocks,
            server_unix_ms: 1_795_000_000_000,
            player: 7,
            spawn: [99_999.5, 64.0, 0.25],
            chunk_radius_m: 512,
            physics: PlayerParams::default(),
            mode: Mode::Walk,
            admin: true,
        });
        let client = ClientMsg::Hello(ClientHello {
            protocol: crate::PROTOCOL_VERSION,
            name: "alice".into(),
            auth: Auth::Token("AQVhbGljZQ".into()),
        });
        assert_eq!(
            from_bytes::<ClientMsg>(&to_bytes(&client).unwrap()).unwrap(),
            client
        );
        let msgs = [
            hello,
            ServerMsg::Blocks {
                chunk: [3_124, -2, 0],
                lsn: 9,
                changes: vec![(0, 1), (32_767, 0)],
            },
            ServerMsg::Pong {
                client_ms: -5,
                server_unix_ms: 1,
            },
            ServerMsg::Chat(Chat::Say {
                from: "alice".into(),
                text: "salut".into(),
            }),
            ServerMsg::Chat(Chat::Admin("redémarrage".into())),
            ServerMsg::Died {
                cause: DeathCause::Suffocation,
            },
            ServerMsg::Respawned {
                pos: [0.5, -12.25, 99_999.75],
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
            cmd: Some(1_000_000),
        })
        .unwrap();
        assert!(edit.len() <= 13, "{} bytes", edit.len());
        for m in [ClientMsg::Respawn, ClientMsg::Fly(true)] {
            assert_eq!(from_bytes::<ClientMsg>(&to_bytes(&m).unwrap()).unwrap(), m);
        }
        let mut b = to_bytes(&ClientMsg::Bye).unwrap();
        b.push(0);
        assert!(from_bytes::<ClientMsg>(&b).is_err());
        assert!(from_bytes::<ClientMsg>(&[200]).is_err());
    }
}
