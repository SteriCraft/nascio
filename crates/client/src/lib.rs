//! Nascio client. For now the headless part: the connection to the server ([`net`]), the
//! mirror of the columns received ([`mirror`]) and the entities around the player
//! ([`entities`]), shared by the future 3D client (S4) and the
//! load-test bots (S6). The client never generates terrain (invariant 5): it only holds what
//! the server sent, and it depends on `nascio-chunk` and never on the generation crates
//! (checked by `tests/no_generation.rs`). The block table comes from the server's hello.

pub mod entities;
pub mod mirror;
pub mod net;

pub use entities::{Entities, Pose};
pub use mirror::Mirror;
pub use nascio_protocol::messages::Auth;
pub use net::{Connection, Event, connect, connect_with};
