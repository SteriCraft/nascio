//! Chantier 2: accounts. Registration is closed by default (an unknown name is refused);
//! a ban, temporary or permanent, refuses the account with its reason and kicks it if
//! online; lifting it lets the player back in.

use std::time::Duration;

use nascio_client::{Event, connect};
use nascio_protocol::messages::ServerMsg;
use nascio_server::net::unix_ms;
use nascio_server::{RunningServer, ServerConfig};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

fn cfg() -> ServerConfig {
    ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        chunk_radius_m: 32,
        generation_threads: 2,
        account_hash_kib: 256,
        account_hash_passes: 1,
        // Refusals below are not guesses, but keep the guard out of the way.
        login_failures_per_min: 100,
        ..ServerConfig::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closed_registration_and_bans() {
    let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world;
    let dir = std::env::temp_dir().join(format!("nascio-accounts-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!cfg().open_registration, "closed by default");
    let server = RunningServer::start(&world, cfg(), &dir).unwrap();
    let (addr, cert) = (server.addr, server.cert_der.clone());
    let sh = &server.shared;

    // Closed registration: an unknown name is refused like a wrong secret.
    let e = connect(addr, &cert, "stranger", "some-secret")
        .await
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("wrong name or secret"), "{e:#}");
    assert!(!sh.accounts.exists("stranger"));
    sh.accounts
        .create("alice", "alice-secret", unix_ms())
        .unwrap();
    sh.accounts.create("bob", "bob-secret!", unix_ms()).unwrap();

    // A temporary ban kicks alice, with its reason, and refuses her next login.
    let mut alice = connect(addr, &cert, "alice", "alice-secret").await.unwrap();
    let (ban, kicked) = sh
        .ban("alice", Some(unix_ms() + 30 * 60_000), "griefing")
        .unwrap();
    assert!(kicked);
    assert_eq!(ban.reason, "griefing");
    let end = tokio::time::Instant::now() + Duration::from_secs(20);
    let kick = loop {
        let left = end.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "no kick");
        match alice.next(left).await {
            Some(Event::Msg(ServerMsg::Kick { reason })) => break reason,
            Some(Event::Closed(r)) => panic!("closed before the kick: {r}"),
            _ => {}
        }
    };
    assert!(kick.contains("banned for 30 min: griefing"), "{kick}");
    drop(alice);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let e = connect(addr, &cert, "alice", "alice-secret")
        .await
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("griefing"), "{e:#}");
    // The ban is told only with the right secret.
    let e = connect(addr, &cert, "alice", "not-the-secret")
        .await
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("wrong name or secret"), "{e:#}");

    // A permanent ban of someone offline; an expired ban no longer counts.
    let (_, kicked) = sh.ban("bob", None, "").unwrap();
    assert!(!kicked);
    let e = connect(addr, &cert, "bob", "bob-secret!")
        .await
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("banned permanently"), "{e:#}");
    sh.ban("bob", Some(unix_ms() - 1), "over").unwrap();
    assert_eq!(sh.accounts.bans(unix_ms()).len(), 1, "only alice");
    let bob = connect(addr, &cert, "bob", "bob-secret!").await.unwrap();
    drop(bob);

    // Lifted: alice is back.
    assert!(sh.accounts.unban("alice", unix_ms()).unwrap());
    let alice = connect(addr, &cert, "alice", "alice-secret").await.unwrap();
    drop(alice);

    // Bans are logged as refused logins.
    let log = std::fs::read_to_string(dir.join("server").join("abuse.log")).unwrap();
    assert!(log.contains("banned"), "{log}");
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
