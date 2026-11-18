//! Connections: handshake on the control stream, then three tasks per player — reader of the
//! control stream, writer of the control stream, and the column streamer (closest columns
//! first, within the radius, under the byte budget of the client).

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
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

use crate::accounts::{Accounts, Login};
use crate::columns::{ColumnCache, wrap_column};
use crate::config::ServerConfig;
use crate::security::{AbuseLog, Bucket, LoginGuard};
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
    pub accounts: Accounts,
    /// Secret hashes computed at once (`account_hash_concurrency`).
    pub hashing: tokio::sync::Semaphore,
    pub abuse: AbuseLog,
    pub logins: LoginGuard,
    /// Players online, by account name.
    pub sessions: Mutex<BTreeMap<String, Session>>,
}

/// A player online.
pub struct Session {
    pub id: u32,
    pub ip: IpAddr,
    pub link: Arc<PlayerLink>,
    conn: quinn::Connection,
}

impl Shared {
    /// Players online: name, id, position.
    pub fn list(&self) -> Vec<(String, u32, [f64; 3])> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(n, s)| {
                let v = s.link.view.lock().unwrap_or_else(|e| e.into_inner()).pos;
                (n.clone(), s.id, v)
            })
            .collect()
    }

    /// Kicks a player (told why, then disconnected); false if not online.
    pub fn kick(&self, name: &str, reason: &str) -> bool {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = sessions.get(name) else {
            return false;
        };
        let _ = s.link.out.send(ServerMsg::Kick {
            reason: reason.to_owned(),
        });
        let (conn, reason) = (s.conn.clone(), reason.to_owned());
        tokio::spawn(async move {
            // Time for the message to leave.
            tokio::time::sleep(Duration::from_millis(200)).await;
            conn.close(2u32.into(), reason.as_bytes());
        });
        true
    }

    /// Bans an account (`until`: `None` is permanent) and kicks it if online; gives the ban
    /// and whether the player was kicked.
    pub fn ban(
        &self,
        name: &str,
        until_unix_ms: Option<i64>,
        reason: &str,
    ) -> anyhow::Result<(crate::accounts::Ban, bool)> {
        let now = unix_ms();
        let ban = self.accounts.ban(name, until_unix_ms, reason, now)?;
        let kicked = self.kick(name, &ban.message(now));
        Ok((ban, kicked))
    }

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
    let ip = conn.remote_address().ip();
    let refuse = match refuse {
        Some(r) => Some(r),
        None => login(&shared, ip, &hello.name, &hello.secret).await.err(),
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
    let name = hello.name.clone();
    let mut sh = shared.hello.clone();
    sh.player = id;
    sh.server_unix_ms = unix_ms();
    send.write_all(&frame(&to_bytes(&ServerMsg::Hello(sh.clone()))?))
        .await?;
    let (out, mut out_rx) = mpsc::unbounded_channel::<ServerMsg>();
    let (dg_tx, mut dg_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let link = Arc::new(PlayerLink {
        id,
        name: hello.name,
        out,
        subs: Mutex::new(BTreeSet::new()),
        view: Mutex::new(View {
            pos: sh.spawn,
            ..View::default()
        }),
        datagrams: Some(dg_tx),
        queued: Default::default(),
        max_datagram: conn.max_datagram_size().unwrap_or(0).into(),
        send_space: usize::MAX.into(),
        entity_bytes: Default::default(),
    });
    {
        let mut sessions = shared.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.insert(
            name.clone(),
            Session {
                id,
                ip,
                link: link.clone(),
                conn: conn.clone(),
            },
        );
    }
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
    // Entity datagrams from the simulation to the connection. When the connection's own
    // buffer is full, quinn drops the oldest: the next tick carries newer states.
    let (c2, l2) = (conn.clone(), link.clone());
    // It also keeps the free space of that buffer fresh for the simulation, which skips a
    // client whose link is full.
    let datagrams = tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_millis(25));
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                d = dg_rx.recv() => {
                    let Some(d) = d else { break };
                    l2.queued.fetch_sub(d.len(), Ordering::Relaxed);
                    l2.max_datagram
                        .store(c2.max_datagram_size().unwrap_or(0), Ordering::Relaxed);
                    match c2.send_datagram(d.into()) {
                        // Larger than the path allows (it shrank): dropped, like a lost one;
                        // the next ones are cut to the new size.
                        Ok(()) | Err(quinn::SendDatagramError::TooLarge) => {}
                        Err(_) => break,
                    }
                }
                _ = every.tick() => {}
            }
            l2.send_space
                .store(c2.datagram_send_buffer_space(), Ordering::Relaxed);
        }
    });

    // Reader of the control stream, with the limits of the client.
    let cfg = &shared.cfg;
    let now = Instant::now();
    let mut control = Bucket::new(cfg.control_msgs_per_s, cfg.control_burst, now);
    let mut edits = Bucket::new(cfg.edits_per_s, cfg.edits_burst, now);
    let mut violations = 0u32;
    // Last accepted position, and when.
    let mut last_move = (sh.spawn, now);
    // Events by kind: the first of each kind is logged, then every hundredth.
    let counts: Mutex<BTreeMap<String, u64>> = Mutex::default();
    let abuse = |kind: &str, detail: &str| {
        let mut c = counts.lock().unwrap_or_else(|e| e.into_inner());
        let n = c.entry(kind.to_owned()).or_insert(0);
        *n += 1;
        if *n == 1 || (*n).is_multiple_of(100) {
            shared
                .abuse
                .log(unix_ms(), ip, &name, kind, &format!("{detail} (#{n})"));
        }
    };
    let result: anyhow::Result<()> = async {
        while let Some(f) = read_frame(&mut recv, &mut de, &mut buf).await? {
            let now = Instant::now();
            if !control.take(now) {
                let msg = from_bytes::<ClientMsg>(&f)?;
                // A backlog sent at once after a stall of the link: positions (the next
                // one supersedes them) and acks (advisory) are dropped without counting
                // toward a kick.
                if matches!(msg, ClientMsg::Move { .. } | ClientMsg::Ack { .. }) {
                    abuse("burst", "position or ack over the limit, dropped");
                    continue;
                }
                violations += 1;
                abuse("flood", "message over the limit");
                if violations >= cfg.kick_after_violations {
                    abuse("kick", "too many messages");
                    let _ = link.out.send(ServerMsg::Kick {
                        reason: "too many messages".into(),
                    });
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    break;
                }
                continue;
            }
            match from_bytes::<ClientMsg>(&f)? {
                ClientMsg::Ping { client_ms } => {
                    let _ = link.out.send(ServerMsg::Pong {
                        client_ms,
                        server_unix_ms: unix_ms(),
                    });
                }
                ClientMsg::Move { pos, yaw, pitch } => {
                    if !pos.iter().all(|v| v.is_finite()) {
                        continue;
                    }
                    let pos = [torus::wrap_f64(pos[0]), pos[1], torus::wrap_f64(pos[2])];
                    let d = distance(last_move.0, pos);
                    let dt = now.saturating_duration_since(last_move.1).as_secs_f64();
                    // Slack for jitter: half a second of motion and two metres.
                    if cfg.max_speed_m_s > 0.0 && d > cfg.max_speed_m_s * (dt + 0.5) + 2.0 {
                        violations += 1;
                        abuse("speed", &format!("{d:.1} m in {dt:.2} s"));
                        let _ = link.out.send(ServerMsg::Correct { pos: last_move.0 });
                        continue;
                    }
                    last_move = (pos, now);
                    *link.view.lock().unwrap_or_else(|e| e.into_inner()) = View { pos, yaw, pitch };
                }
                ClientMsg::SetBlock { pos, block } => {
                    let refused = if !edits.take(now) {
                        violations += 1;
                        abuse("edits", "edit over the limit");
                        Some("too many edits".to_owned())
                    } else {
                        let at = [
                            f64::from(pos[0]) + 0.5,
                            f64::from(pos[1]) + 0.5,
                            f64::from(pos[2]) + 0.5,
                        ];
                        let d = distance(last_move.0, at);
                        (cfg.edit_reach_m > 0.0 && d > cfg.edit_reach_m).then(|| {
                            violations += 1;
                            abuse("reach", &format!("edit {d:.0} m away"));
                            format!("out of reach ({d:.0} m)")
                        })
                    };
                    match refused {
                        Some(reason) => {
                            let _ = link.out.send(ServerMsg::EditRefused { pos, reason });
                        }
                        None => shared.input(Input::Msg(id, ClientMsg::SetBlock { pos, block })),
                    }
                    if violations >= cfg.kick_after_violations {
                        abuse("kick", "too many refused messages");
                        let _ = link.out.send(ServerMsg::Kick {
                            reason: "too many refused messages".into(),
                        });
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        break;
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
    {
        let mut sessions = shared.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if sessions.get(&name).is_some_and(|s| s.id == id) {
            sessions.remove(&name);
        }
    }
    shared.input(Input::Leave(id));
    shared.online.fetch_sub(1, Ordering::Relaxed);
    streamer.abort();
    writer.abort();
    datagrams.abort();
    conn.close(0u32.into(), b"bye");
    result
}

/// Distance on the torus, metres.
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dz) = (torus::delta_f64(a[0], b[0]), torus::delta_f64(a[2], b[2]));
    (dx * dx + (b[1] - a[1]).powi(2) + dz * dz).sqrt()
}

/// Checks a login (hashing on a blocking thread); `Err` holds the reason given to the client.
async fn login(shared: &Arc<Shared>, ip: IpAddr, name: &str, secret: &str) -> Result<(), String> {
    let cfg = &shared.cfg;
    let now = Instant::now();
    if !shared.logins.allowed(ip, cfg.login_failures_per_min, now) {
        shared
            .abuse
            .log(unix_ms(), ip, name, "login", "blocked: too many failures");
        return Err("too many failed logins: wait a minute".into());
    }
    let (sh, n, s, open) = (
        shared.clone(),
        name.to_owned(),
        secret.to_owned(),
        cfg.open_registration,
    );
    // A rush of logins waits here rather than taking every core.
    let _permit = shared.hashing.acquire().await.map_err(|e| format!("{e}"))?;
    let r = tokio::task::spawn_blocking(move || sh.accounts.login(&n, &s, open, unix_ms()))
        .await
        .map_err(|e| format!("{e}"))?;
    match r {
        Ok(login) => {
            if login == Login::Created {
                println!("account {name} created ({ip})");
            }
            // Only once the secret is right, like the session check below.
            if let Some(ban) = shared.accounts.banned(name, unix_ms()) {
                shared.abuse.log(unix_ms(), ip, name, "login", "banned");
                return Err(ban.message(unix_ms()));
            }
            // Only once the secret is right: a stranger does not learn who is online.
            if shared
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(name)
            {
                return Err(format!("{name} is already connected"));
            }
            Ok(())
        }
        Err(e) => {
            shared.logins.failed(ip, now);
            let e = format!("{e:#}");
            shared.abuse.log(unix_ms(), ip, name, "login", &e);
            // Do not tell which of the two was wrong.
            Err(if e == "unknown account" || e == "wrong secret" {
                "wrong name or secret".into()
            } else {
                e
            })
        }
    }
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
    // Column of the player when its ring was last found complete: nothing to scan until it
    // moves to another column.
    let mut complete: Option<(i32, i32)> = None;
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
        // Entities came first out of the same budget.
        tokens -= link.entity_bytes.swap(0, Ordering::Relaxed) as f64;
        let now = Instant::now();
        tokens = (tokens + budget * (now - last).as_secs_f64()).min(budget);
        last = now;
        let pos = link.view.lock().unwrap_or_else(|e| e.into_inner()).pos;
        let cs = f64::from(CHUNK_SIZE);
        let (pcx, pcz) = wrap_column(
            (pos[0].rem_euclid(WORLD_SIZE_F64) / cs).floor() as i32,
            (pos[2].rem_euclid(WORLD_SIZE_F64) / cs).floor() as i32,
        );
        if complete == Some((pcx, pcz)) {
            continue;
        }
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
            let Some((cx, cz)) = next else {
                complete = Some((pcx, pcz));
                break;
            };
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
