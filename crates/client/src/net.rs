//! Connection to a server over QUIC: a control stream (framed `postcard` messages) and one
//! unidirectional stream per column, horizon tile or `WorldSky` sent by the server (a
//! [`UniKind`] byte first).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, bail};
use nascio_chunk::horizon::HorizonTile;
use nascio_chunk::{BlockRegistry, Blocks};
use nascio_protocol::UniKind;
use nascio_protocol::chunk::{Column, MAX_COLUMN_BYTES};
use nascio_protocol::frame::{Deframer, MAX_CONTROL_FRAME, frame};
use nascio_protocol::horizon::{MAX_TILE_BYTES, decode_tile};
use nascio_protocol::messages::{
    Auth, ClientHello, ClientMsg, ServerHello, ServerMsg, from_bytes, to_bytes,
};
use nascio_protocol::sky::{MAX_SKY_BYTES, WorldSky};
use quinn::rustls::RootCertStore;
use quinn::rustls::pki_types::CertificateDer;
use tokio::sync::mpsc;

/// What the server sends after the handshake.
#[derive(Debug)]
pub enum Event {
    Msg(ServerMsg),
    Column(Box<Column>),
    /// A horizon tile asked with `ClientMsg::Tiles`, with its compressed bytes (for the disk
    /// cache).
    Tile(Box<HorizonTile>, Vec<u8>),
    /// The sky and far ground asked with `ClientMsg::Sky`, with its compressed bytes (for the
    /// disk cache).
    Sky(Box<WorldSky>, Vec<u8>),
    /// An entity datagram (decode with [`crate::Entities`], then acknowledge its seq).
    Entities(Vec<u8>),
    /// The connection ended (reason).
    Closed(String),
}

/// An open connection.
pub struct Connection {
    pub hello: ServerHello,
    /// The server's block table (from `hello.blocks`, validated), for meshing.
    pub blocks: Blocks,
    conn: quinn::Connection,
    out: mpsc::UnboundedSender<ClientMsg>,
    pub events: mpsc::UnboundedReceiver<Event>,
    /// Compressed column bytes received (statistics).
    pub column_bytes: Arc<AtomicU64>,
    _endpoint: quinn::Endpoint,
}

impl Connection {
    /// Queues a message on the control stream.
    pub fn send(&self, m: ClientMsg) {
        let _ = self.out.send(m);
    }

    /// The next event, or `None` after `timeout`.
    pub async fn next(&mut self, timeout: Duration) -> Option<Event> {
        tokio::time::timeout(timeout, self.events.recv())
            .await
            .ok()
            .flatten()
    }

    pub fn close(&self) {
        self.send(ClientMsg::Bye);
        self.conn.close(0u32.into(), b"bye");
    }

    /// Round trip time measured by QUIC.
    pub fn rtt(&self) -> Duration {
        self.conn.rtt()
    }
}

/// Connects and logs in. `cert_der` is the server's certificate, trusted as is
/// (self-signed server certificate, pinned by the client).
pub async fn connect(
    addr: SocketAddr,
    cert_der: &[u8],
    name: &str,
    secret: &str,
) -> anyhow::Result<Connection> {
    connect_with(addr, cert_der, name, Auth::Secret(secret.to_owned())).await
}

/// Connects with a secret or a token from the site.
pub async fn connect_with(
    addr: SocketAddr,
    cert_der: &[u8],
    name: &str,
    auth: Auth,
) -> anyhow::Result<Connection> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(cert_der.to_vec()))?;
    let mut cfg = quinn::ClientConfig::with_root_certificates(Arc::new(roots))?;
    let mut transport = quinn::TransportConfig::default();
    transport
        .max_concurrent_uni_streams(1024u32.into())
        .keep_alive_interval(Some(Duration::from_secs(5)));
    cfg.transport_config(Arc::new(transport));
    let bind: SocketAddr = if addr.is_ipv6() {
        "[::]:0".parse()?
    } else {
        "0.0.0.0:0".parse()?
    };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    endpoint.set_default_client_config(cfg);
    let conn = endpoint
        .connect(addr, "localhost")?
        .await
        .context("connecting")?;
    let (mut send, mut recv) = conn.open_bi().await?;
    let hello = ClientMsg::Hello(ClientHello {
        protocol: nascio_protocol::PROTOCOL_VERSION,
        name: name.to_owned(),
        auth,
    });
    send.write_all(&frame(&to_bytes(&hello)?)).await?;
    let mut de = Deframer::new(MAX_CONTROL_FRAME);
    let first = loop {
        if let Some(f) = de.next_frame()? {
            break f;
        }
        let mut buf = [0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(10), recv.read(&mut buf))
            .await
            .context("handshake timed out")??
            .context("server closed the control stream")?;
        de.push(&buf[..n]);
    };
    let hello = match from_bytes::<ServerMsg>(&first)? {
        ServerMsg::Hello(h) => h,
        ServerMsg::Refused { reason } => bail!("refused by the server: {reason}"),
        m => bail!("unexpected first message {m:?}"),
    };
    let blocks = Blocks::new(BlockRegistry {
        blocks: hello.blocks.clone(),
    })
    .context("the server's block table")?;
    let max_id = u16::try_from(blocks.len().saturating_sub(1)).unwrap_or(u16::MAX);
    let (out, mut out_rx) = mpsc::unbounded_channel::<ClientMsg>();
    let (ev, events) = mpsc::unbounded_channel();
    // Writer of the control stream.
    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            let Ok(b) = to_bytes(&m) else { break };
            if send.write_all(&frame(&b)).await.is_err() {
                break;
            }
        }
        let _ = send.finish();
    });
    // Reader of the control stream.
    let ev2 = ev.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 16 << 10];
        let reason = loop {
            loop {
                match de.next_frame() {
                    Ok(Some(f)) => match from_bytes::<ServerMsg>(&f) {
                        Ok(m) => {
                            let _ = ev2.send(Event::Msg(m));
                        }
                        Err(e) => return drop(ev2.send(Event::Closed(format!("{e:#}")))),
                    },
                    Ok(None) => break,
                    Err(e) => return drop(ev2.send(Event::Closed(format!("{e:#}")))),
                }
            }
            match recv.read(&mut buf).await {
                Ok(Some(n)) => de.push(&buf[..n]),
                Ok(None) => break "control stream ended".to_owned(),
                Err(e) => break format!("{e}"),
            }
        };
        let _ = ev2.send(Event::Closed(reason));
    });
    // Entity datagrams.
    let (c3, ev3) = (conn.clone(), ev.clone());
    tokio::spawn(async move {
        while let Ok(d) = c3.read_datagram().await {
            if ev3.send(Event::Entities(d.to_vec())).is_err() {
                break;
            }
        }
    });
    // Columns, tiles and the sky, one unidirectional stream each.
    let c2 = conn.clone();
    let column_bytes = Arc::new(AtomicU64::new(0));
    let cb = column_bytes.clone();
    tokio::spawn(async move {
        while let Ok(mut s) = c2.accept_uni().await {
            let (ev, cb) = (ev.clone(), cb.clone());
            tokio::spawn(async move {
                let mut kind = [0u8; 1];
                if s.read_exact(&mut kind).await.is_err() {
                    return;
                }
                let Some(kind) = UniKind::from_u8(kind[0]) else {
                    let _ = ev.send(Event::Closed(format!("unknown stream kind {}", kind[0])));
                    return;
                };
                let limit = match kind {
                    UniKind::Column => MAX_COLUMN_BYTES,
                    UniKind::Tile => MAX_TILE_BYTES,
                    UniKind::Sky => MAX_SKY_BYTES,
                };
                let Ok(bytes) = s.read_to_end(limit).await else {
                    return;
                };
                cb.fetch_add(bytes.len() as u64 + 1, Ordering::Relaxed);
                let event = match kind {
                    UniKind::Column => Column::decompress(&bytes, max_id)
                        .map(|c| Event::Column(Box::new(c)))
                        .map_err(|e| format!("bad column: {e:#}")),
                    UniKind::Tile => decode_tile(&bytes)
                        .map(|t| Event::Tile(Box::new(t), bytes))
                        .map_err(|e| format!("bad horizon tile: {e:#}")),
                    UniKind::Sky => WorldSky::decompress(&bytes)
                        .map(|s| Event::Sky(Box::new(s), bytes))
                        .map_err(|e| format!("bad sky: {e:#}")),
                };
                let _ = ev.send(event.unwrap_or_else(Event::Closed));
            });
        }
    });
    Ok(Connection {
        hello,
        blocks,
        conn,
        out,
        events,
        column_bytes,
        _endpoint: endpoint,
    })
}
