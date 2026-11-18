//! Connection to a server over QUIC: a control stream (framed `postcard` messages) and one
//! unidirectional stream per column sent by the server.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use nascio_protocol::chunk::{Column, MAX_COLUMN_BYTES};
use nascio_protocol::frame::{Deframer, MAX_CONTROL_FRAME, frame};
use nascio_protocol::messages::{
    ClientHello, ClientMsg, ServerHello, ServerMsg, from_bytes, to_bytes,
};
use quinn::rustls::RootCertStore;
use quinn::rustls::pki_types::CertificateDer;
use tokio::sync::mpsc;

/// What the server sends after the handshake.
#[derive(Debug)]
pub enum Event {
    Msg(ServerMsg),
    Column(Box<Column>),
    /// An entity datagram (decode with [`crate::Entities`], then acknowledge its seq).
    Entities(Vec<u8>),
    /// The connection ended (reason).
    Closed(String),
}

/// An open connection.
pub struct Connection {
    pub hello: ServerHello,
    conn: quinn::Connection,
    out: mpsc::UnboundedSender<ClientMsg>,
    pub events: mpsc::UnboundedReceiver<Event>,
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

/// Connects and shakes hands. `cert_der` is the server's certificate, trusted as is
/// (self-signed server certificate, pinned by the client).
pub async fn connect(addr: SocketAddr, cert_der: &[u8], name: &str) -> anyhow::Result<Connection> {
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
    let max_id = u16::try_from(hello.blocks.len().saturating_sub(1)).unwrap_or(u16::MAX);
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
    // Columns, one unidirectional stream each.
    let c2 = conn.clone();
    tokio::spawn(async move {
        while let Ok(mut s) = c2.accept_uni().await {
            let ev = ev.clone();
            tokio::spawn(async move {
                let Ok(bytes) = s.read_to_end(MAX_COLUMN_BYTES).await else {
                    return;
                };
                match Column::decompress(&bytes, max_id) {
                    Ok(c) => {
                        let _ = ev.send(Event::Column(Box::new(c)));
                    }
                    Err(e) => {
                        let _ = ev.send(Event::Closed(format!("bad column: {e:#}")));
                    }
                }
            });
        }
    });
    Ok(Connection {
        hello,
        conn,
        out,
        events,
        _endpoint: endpoint,
    })
}
