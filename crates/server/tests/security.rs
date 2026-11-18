//! S7 validation: abuses are refused and logged — wrong secret, repeated failed logins, a
//! second session, message and edit floods, edits out of reach, teleports — and the
//! administrator can kick a player. A backlog of positions and acks sent at once (after a
//! stall of the link) is dropped without a kick.

use std::time::Duration;

use nascio_client::{Connection, Event, connect};
use nascio_protocol::messages::{ClientMsg, ServerMsg};
use nascio_server::{RunningServer, ServerConfig};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

fn cfg() -> ServerConfig {
    ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        chunk_radius_m: 64,
        generation_threads: 2,
        // Cheap hashes for the test.
        account_hash_kib: 256,
        account_hash_passes: 1,
        // One hash at a time: logins queue.
        account_hash_concurrency: 1,
        login_failures_per_min: 3,
        control_msgs_per_s: 50.0,
        control_burst: 100.0,
        edits_per_s: 5.0,
        edits_burst: 10.0,
        kick_after_violations: 300,
        max_speed_m_s: 20.0,
        edit_reach_m: 16.0,
        ..ServerConfig::default()
    }
}

/// The next server message matching `want`, or panics after `secs` seconds.
async fn wait_for(
    c: &mut Connection,
    secs: u64,
    mut want: impl FnMut(&ServerMsg) -> bool,
) -> ServerMsg {
    let end = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let left = end.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "timed out");
        match c.next(left).await {
            Some(Event::Msg(m)) if want(&m) => return m,
            Some(Event::Closed(r)) => panic!("closed: {r}"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abuses_are_refused_and_logged() {
    let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world;
    let dir = std::env::temp_dir().join(format!("nascio-security-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let server = RunningServer::start(&world, cfg(), &dir).unwrap();
    let (addr, cert) = (server.addr, server.cert_der.clone());

    // Accounts: created on first login; a wrong secret is refused without saying which part
    // was wrong; a second session is refused.
    let mut alice = connect(addr, &cert, "alice", "alice-secret").await.unwrap();
    // Logged in before the failures below (they block this address for a minute).
    let mut dave = connect(addr, &cert, "dave", "dave-secret").await.unwrap();
    let mut erin = connect(addr, &cert, "erin", "erin-secret").await.unwrap();
    assert!(server.shared.accounts.exists("alice"));
    let e = connect(addr, &cert, "alice", "not-her-secret")
        .await
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("wrong name or secret"), "{e:#}");
    let e = connect(addr, &cert, "alice", "alice-secret")
        .await
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("already connected"), "{e:#}");
    assert!(
        connect(addr, &cert, "x", "whatever-secret").await.is_err(),
        "bad name"
    );
    // Three failures from this address in a minute: even the right secret waits.
    for _ in 0..2 {
        let _ = connect(addr, &cert, "carol", "short").await;
    }
    let e = connect(addr, &cert, "bob", "bob-secret!")
        .await
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("too many failed logins"), "{e:#}");

    // Edits: out of reach, then too many at once.
    let spawn = alice.hello.spawn;
    let base = [spawn[0] as i32, spawn[1] as i32 + 3, spawn[2] as i32];
    alice.send(ClientMsg::SetBlock {
        pos: [base[0] + 100, base[1], base[2]],
        block: 1,
    });
    let m = wait_for(&mut alice, 20, |m| {
        matches!(m, ServerMsg::EditRefused { .. })
    })
    .await;
    assert!(format!("{m:?}").contains("out of reach"), "{m:?}");
    for k in 0..30 {
        alice.send(ClientMsg::SetBlock {
            pos: [base[0] + k % 4, base[1], base[2]],
            block: 0,
        });
    }
    let m = wait_for(
        &mut alice,
        20,
        |m| matches!(m, ServerMsg::EditRefused { reason, .. } if reason.contains("too many")),
    )
    .await;
    assert!(format!("{m:?}").contains("too many edits"));

    // A teleport: refused, and the player is put back.
    alice.send(ClientMsg::Move {
        pos: [spawn[0] + 5_000.0, spawn[1], spawn[2]],
        yaw: 0.0,
        pitch: 0.0,
    });
    let m = wait_for(&mut alice, 20, |m| matches!(m, ServerMsg::Correct { .. })).await;
    let ServerMsg::Correct { pos } = m else {
        unreachable!()
    };
    assert!((pos[0] - spawn[0]).abs() < 1.0 && (pos[2] - spawn[2]).abs() < 1.0);
    let listed = server.shared.list();
    assert!((listed[0].2[0] - spawn[0]).abs() < 1.0, "{listed:?}");

    // The administrator kicks alice.
    assert!(server.shared.kick("alice", "test over"));
    let m = wait_for(&mut alice, 20, |m| matches!(m, ServerMsg::Kick { .. })).await;
    assert!(format!("{m:?}").contains("test over"));

    // A backlog of positions and acks after a stall of the link: dropped, never a kick
    // (the flood below, of pings, is kicked after 300 refusals).
    let at = erin.hello.spawn;
    for k in 0..1_000 {
        erin.send(ClientMsg::Move {
            pos: [at[0] + f64::from(k % 2), at[1], at[2]],
            yaw: 0.0,
            pitch: 0.0,
        });
        erin.send(ClientMsg::Ack { seqs: vec![k] });
    }
    // The bucket refills (50 per second); erin still answers.
    tokio::time::sleep(Duration::from_secs(3)).await;
    erin.send(ClientMsg::Ping { client_ms: 7 });
    let m = wait_for(&mut erin, 20, |m| {
        matches!(m, ServerMsg::Pong { .. } | ServerMsg::Kick { .. })
    })
    .await;
    assert!(matches!(m, ServerMsg::Pong { client_ms: 7, .. }), "{m:?}");

    // A flood of messages: dropped, then the client is kicked.
    for k in 0..2_000 {
        dave.send(ClientMsg::Ping { client_ms: k });
    }
    let m = wait_for(&mut dave, 30, |m| matches!(m, ServerMsg::Kick { .. })).await;
    assert!(format!("{m:?}").contains("too many messages"));

    // Everything is in the abuse log.
    let log = std::fs::read_to_string(dir.join("server").join("abuse.log")).unwrap();
    for kind in ["login", "reach", "edits", "speed", "flood", "kick", "burst"] {
        assert!(
            log.lines().any(|l| l.split('\t').nth(3) == Some(kind)),
            "no {kind} in the log:\n{log}"
        );
    }
    assert!(!log.contains("alice-secret"), "secrets are not logged");
    eprintln!("abuse log:\n{log}");
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
