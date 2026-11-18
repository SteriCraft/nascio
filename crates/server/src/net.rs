//! Connections: handshake on the control stream, then three tasks per player — reader of the
//! control stream, writer of the control stream, and the column streamer (closest columns
//! first, within the radius, under the byte budget of the client).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use nascio_core::consts::{CHUNK_SIZE, WORLD_SIZE_F64};
use nascio_core::torus;
use nascio_protocol::frame::{Deframer, MAX_CONTROL_FRAME, frame};
use nascio_protocol::messages::{ClientMsg, ServerHello, ServerMsg, from_bytes, to_bytes};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::columns::{ColumnCache, wrap_column};
use crate::config::ServerConfig;
use crate::sim::{Input, PlayerLink, View};

/// What every connection needs.
pub struct Shared {
    pub cfg: ServerConfig,
    pub hello: ServerHello,
    pub cache: Arc<ColumnCache>,
    pub inputs: Mutex<Sender<Input>>,
    pub next_id: AtomicU32,
    pub online: AtomicU32,
    /// Column bytes sent (statistics).
    pub sent_bytes: AtomicU64,
    pub sent_columns: AtomicU64,
}

impl Shared {
    fn input(&self, i: Input) {
        let _ = self
            .inputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(i);
    }
}

pub fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Accepts connections until the endpoint closes.
pub async fn accept_loop(endpoint: quinn::Endpoint, shared: Arc<Shared>) {
    while let Some(incoming) = endpoint.accept().await {
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = connection(incoming, shared).await {
                eprintln!("connection: {e:#}");
            }
        });
    }
}

async fn read_frame(
    recv: &mut quinn::RecvStream,
    de: &mut Deframer,
    buf: &mut [u8],
) -> anyhow::Result<Option<Vec<u8>>> {
    loop {
        if let Some(f) = de.next_frame()? {
            return Ok(Some(f));
        }
        match recv.read(buf).await {
            Ok(Some(n)) => de.push(&buf[..n]),
            Ok(None) => return Ok(None),
            // The client closed the connection: a normal end.
            Err(quinn::ReadError::ConnectionLost(
                quinn::ConnectionError::ApplicationClosed(_)
                | quinn::ConnectionError::LocallyClosed,
            )) => {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        }
    }
}

async fn connection(incoming: quinn::Incoming, shared: Arc<Shared>) -> anyhow::Result<()> {
    let conn = incoming.await?;
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(10), conn.accept_bi())
        .await
        .context("no control stream")??;
    let mut de = Deframer::new(MAX_CONTROL_FRAME);
    let mut buf = vec![0u8; 16 << 10];
    let first = tokio::time::timeout(
        Duration::from_secs(10),
        read_frame(&mut recv, &mut de, &mut buf),
    )
    .await
    .context("no hello")??
    .context("closed before hello")?;
    let hello = match from_bytes::<ClientMsg>(&first)? {
        ClientMsg::Hello(h) => h,
        m => bail!("unexpected first message {m:?}"),
    };
    let refuse = if hello.protocol != nascio_protocol::PROTOCOL_VERSION {
        Some(format!(
            "protocol {} (server: {})",
            hello.protocol,
            nascio_protocol::PROTOCOL_VERSION
        ))
    } else if shared.online.load(Ordering::Relaxed) as usize >= shared.cfg.max_players {
        Some("server full".to_owned())
    } else {
        None
    };
    if let Some(reason) = refuse {
        send.write_all(&frame(&to_bytes(&ServerMsg::Refused {
            reason: reason.clone(),
        })?))
        .await?;
        let _ = send.finish();
        tokio::time::sleep(Duration::from_millis(200)).await;
        conn.close(1u32.into(), reason.as_bytes());
        return Ok(());
    }
    let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
    let mut sh = shared.hello.clone();
    sh.player = id;
    sh.server_unix_ms = unix_ms();
    send.write_all(&frame(&to_bytes(&ServerMsg::Hello(sh.clone()))?))
        .await?;
    let (out, mut out_rx) = mpsc::unbounded_channel::<ServerMsg>();
    let link = Arc::new(PlayerLink {
        id,
        name: hello.name,
        out,
        subs: Mutex::new(BTreeSet::new()),
        view: Mutex::new(View {
            pos: sh.spawn,
            ..View::default()
        }),
    });
    shared.online.fetch_add(1, Ordering::Relaxed);
    shared.input(Input::Join(link.clone()));

    // Writer of the control stream.
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            let Ok(b) = to_bytes(&m) else { break };
            if send.write_all(&frame(&b)).await.is_err() {
                break;
            }
        }
    });
    let streamer = tokio::spawn(stream_columns(conn.clone(), link.clone(), shared.clone()));

    // Reader of the control stream.
    let result: anyhow::Result<()> = async {
        while let Some(f) = read_frame(&mut recv, &mut de, &mut buf).await? {
            match from_bytes::<ClientMsg>(&f)? {
                ClientMsg::Ping { client_ms } => {
                    let _ = link.out.send(ServerMsg::Pong {
                        client_ms,
                        server_unix_ms: unix_ms(),
                    });
                }
                ClientMsg::Move { pos, yaw, pitch } => {
                    if pos.iter().all(|v| v.is_finite()) {
                        *link.view.lock().unwrap_or_else(|e| e.into_inner()) = View {
                            pos: [torus::wrap_f64(pos[0]), pos[1], torus::wrap_f64(pos[2])],
                            yaw,
                            pitch,
                        };
                    }
                }
                ClientMsg::Bye => break,
                ClientMsg::Hello(_) => bail!("second hello"),
                m => shared.input(Input::Msg(id, m)),
            }
        }
        Ok(())
    }
    .await;
    shared.input(Input::Leave(id));
    shared.online.fetch_sub(1, Ordering::Relaxed);
    streamer.abort();
    writer.abort();
    conn.close(0u32.into(), b"bye");
    result
}

/// Column offsets within `radius` columns, closest first (ties in a fixed order).
fn offsets(radius: i32) -> Vec<(i32, i32)> {
    let mut v: Vec<(i32, i32)> = (-radius..=radius)
        .flat_map(|dz| (-radius..=radius).map(move |dx| (dx, dz)))
        .filter(|&(dx, dz)| dx * dx + dz * dz <= radius * radius)
        .collect();
    v.sort_by_key(|&(dx, dz)| (dx * dx + dz * dz, dz, dx));
    v
}

/// Sends the columns around the player, closest first, under the byte budget; forgets the
/// columns that went out of range.
async fn stream_columns(conn: quinn::Connection, link: Arc<PlayerLink>, shared: Arc<Shared>) {
    let cfg = &shared.cfg;
    let radius = (cfg.chunk_radius_m as i32 + CHUNK_SIZE - 1) / CHUNK_SIZE;
    let keep = radius + cfg.forget_margin_m as i32 / CHUNK_SIZE;
    let ring = offsets(radius);
    let budget = f64::from(cfg.client_budget_bytes_per_s);
    // Token bucket: at most one second of budget in advance.
    let mut tokens = budget;
    let mut last = Instant::now();
    let mut flight: JoinSet<anyhow::Result<usize>> = JoinSet::new();
    loop {
        tokio::select! {
            _ = conn.closed() => return,
            Some(done) = flight.join_next(), if !flight.is_empty() => {
                match done {
                    Ok(Ok(n)) => tokens -= n as f64,
                    Ok(Err(e)) => { eprintln!("column for player {}: {e:#}", link.id); return; }
                    Err(_) => return,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        let now = Instant::now();
        tokens = (tokens + budget * (now - last).as_secs_f64()).min(budget);
        last = now;
        let pos = link.view.lock().unwrap_or_else(|e| e.into_inner()).pos;
        let cs = f64::from(CHUNK_SIZE);
        let (pcx, pcz) = wrap_column(
            (pos[0].rem_euclid(WORLD_SIZE_F64) / cs).floor() as i32,
            (pos[2].rem_euclid(WORLD_SIZE_F64) / cs).floor() as i32,
        );
        // Forget what went out of range.
        let gone: Vec<[i32; 2]> = {
            let mut subs = link.subs.lock().unwrap_or_else(|e| e.into_inner());
            let out: Vec<(i32, i32)> = subs
                .iter()
                .filter(|&&(cx, cz)| {
                    let (dx, dz) = (torus::delta_chunks(pcx, cx), torus::delta_chunks(pcz, cz));
                    dx * dx + dz * dz > keep * keep
                })
                .copied()
                .collect();
            for k in &out {
                subs.remove(k);
            }
            out.into_iter().map(|(x, z)| [x, z]).collect()
        };
        if !gone.is_empty() {
            let _ = link.out.send(ServerMsg::Forget { columns: gone });
        }
        // Next columns, closest first.
        while tokens > 0.0 && flight.len() < cfg.columns_in_flight {
            let next = {
                let mut subs = link.subs.lock().unwrap_or_else(|e| e.into_inner());
                let k = ring
                    .iter()
                    .map(|&(dx, dz)| wrap_column(pcx + dx, pcz + dz))
                    .find(|k| !subs.contains(k));
                // Subscribed before it is read: no block change of it can be missed.
                if let Some(k) = k {
                    subs.insert(k);
                }
                k
            };
            let Some((cx, cz)) = next else { break };
            // Reserve an average column; corrected when the column is sent.
            tokens -= 1_024.0;
            let conn = conn.clone();
            let shared = shared.clone();
            flight.spawn(async move {
                let bytes = shared.cache.get(cx, cz).await?;
                let mut s = conn.open_uni().await?;
                s.write_all(&bytes).await?;
                s.finish()?;
                shared.sent_columns.fetch_add(1, Ordering::Relaxed);
                shared
                    .sent_bytes
                    .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                Ok(bytes.len().saturating_sub(1_024))
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_closest_first_and_round() {
        let o = offsets(16);
        assert_eq!(o[0], (0, 0));
        assert!(o.windows(2).all(|w| {
            let d = |p: (i32, i32)| p.0 * p.0 + p.1 * p.1;
            d(w[0]) <= d(w[1])
        }));
        // About π·16² columns.
        assert!((780..830).contains(&o.len()), "{}", o.len());
    }
}
