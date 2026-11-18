//! Nascio client. For now the headless part: the connection to the server ([`net`]) and the
//! mirror of the columns received ([`mirror`]), shared by the future 3D client (S4) and the
//! load-test bots (S6). The client never generates terrain (invariant 5): it only holds what
//! the server sent.

pub mod mirror;
pub mod net;

pub use mirror::Mirror;
pub use net::{Connection, Event, connect};
