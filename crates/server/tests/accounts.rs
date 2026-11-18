//! Chantier 2: accounts.
//! - Registration is closed by default (an unknown name is refused); a ban, temporary or
//!   permanent, refuses the account with its reason and kicks it if online; lifting it lets
//!   the player back in.
//! - Tokens from the site: a valid one creates the account and logs in, once; a token for
//!   another server or name, expired, from another key or issued before the server started
//!   is refused — also after a restart, which forgets the spent nonces. Secret logins are
//!   for administrators (or nobody).

use std::time::Duration;

use nascio_client::{Auth, Event, connect, connect_with};
use nascio_protocol::messages::ServerMsg;
use nascio_server::config::SecretLogin;
use nascio_server::net::unix_ms;
use nascio_server::token::{Claims, SKEW_MS, SiteKey, TOKEN_VERSION, b64_encode, generate_key};
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
    assert_eq!(
        cfg().secret_login,
        SecretLogin::Admin,
        "admins only by default"
    );
    // Secrets for everyone here: this part is about registration and bans.
    let cfg = ServerConfig {
        secret_login: SecretLogin::All,
        ..cfg()
    };
    let server = RunningServer::start(&world, cfg, &dir).unwrap();
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

/// Claims for `name` on the test server, issued at `issued`, valid one minute.
fn claims(name: &str, issued: i64, nonce: u8) -> Claims {
    Claims {
        version: TOKEN_VERSION,
        name: name.into(),
        server: "test-1".into(),
        issued_unix_ms: issued,
        expires_unix_ms: issued + 60_000,
        nonce: [nonce; 16],
    }
}

/// The reason a login is refused.
async fn refused(addr: std::net::SocketAddr, cert: &[u8], name: &str, auth: Auth) -> String {
    let e = connect_with(addr, cert, name, auth).await.err().unwrap();
    format!("{e:#}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn site_tokens() {
    let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
        .expect("world")
        .world;
    let dir = std::env::temp_dir().join(format!("nascio-tokens-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (pkcs8, public) = generate_key().unwrap();
    let site = SiteKey::from_pkcs8(&pkcs8).unwrap();
    let cfg = ServerConfig {
        site_public_key: b64_encode(&public),
        server_id: "test-1".into(),
        ..cfg()
    };
    let server = RunningServer::start(&world, cfg.clone(), &dir).unwrap();
    let (addr, cert) = (server.addr, server.cert_der.clone());
    let sh = &server.shared;
    // The server just started: tokens must bear `start + SKEW_MS` at least, as from a site
    // whose clock runs SKEW_MS ahead (with a synchronised site, one waits SKEW_MS).
    let ahead = || unix_ms() + SKEW_MS;
    let token = |c: &Claims| Auth::Token(site.sign(c).unwrap());

    // A fresh token issued right now, before start + SKEW_MS: refused.
    let fresh = site.issue("alice", "test-1", unix_ms(), 60_000).unwrap();
    let e = refused(addr, &cert, "alice", Auth::Token(fresh)).await;
    assert!(e.contains("before the server started"), "{e}");

    // A valid token creates alice's account and logs her in; the same token again: refused.
    let t = token(&claims("alice", ahead(), 1));
    assert!(!sh.accounts.exists("alice"));
    let alice = connect_with(addr, &cert, "alice", t.clone()).await.unwrap();
    assert!(sh.accounts.exists("alice"));
    drop(alice);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let e = refused(addr, &cert, "alice", t).await;
    assert!(e.contains("already used"), "{e}");
    // A second token: alice is back (the account exists).
    let alice = connect_with(addr, &cert, "alice", token(&claims("alice", ahead(), 2)))
        .await
        .unwrap();

    // Refused tokens: another server, another name, expired, another key, junk.
    let mut c = claims("bob", ahead(), 3);
    c.server = "test-2".into();
    let e = refused(addr, &cert, "bob", token(&c)).await;
    assert!(e.contains("another server"), "{e}");
    let e = refused(addr, &cert, "bob", token(&claims("alice", ahead(), 4))).await;
    assert!(e.contains("another name"), "{e}");
    let e = refused(
        addr,
        &cert,
        "bob",
        token(&claims("bob", ahead() - 10 * 60_000, 5)),
    )
    .await;
    assert!(e.contains("expired"), "{e}");
    let (other, _) = generate_key().unwrap();
    let other = SiteKey::from_pkcs8(&other).unwrap();
    let forged = Auth::Token(other.sign(&claims("bob", ahead(), 6)).unwrap());
    let e = refused(addr, &cert, "bob", forged).await;
    assert!(e.contains("not signed by the site"), "{e}");
    let e = refused(addr, &cert, "bob", Auth::Token("garbage!".into())).await;
    assert!(e.contains("malformed"), "{e}");
    assert!(!sh.accounts.exists("bob"));

    // A token account has no secret; secrets are for administrators.
    let e = refused(addr, &cert, "alice", Auth::Secret("anything-at-all".into())).await;
    assert!(e.contains("wrong name or secret"), "{e}");
    sh.accounts
        .create("carol", "carol-secret", unix_ms())
        .unwrap();
    let e = refused(addr, &cert, "carol", Auth::Secret("carol-secret".into())).await;
    assert!(e.contains("for administrators"), "{e}");
    sh.accounts.set_admin("carol", true).unwrap();
    let carol = connect(addr, &cert, "carol", "carol-secret").await.unwrap();
    drop(carol);

    // A banned account: its token is refused with the ban.
    let (_, kicked) = sh.ban("alice", None, "cheating").unwrap();
    assert!(kicked);
    drop(alice);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let e = refused(addr, &cert, "alice", token(&claims("alice", ahead(), 7))).await;
    assert!(e.contains("cheating"), "{e}");
    let log = std::fs::read_to_string(dir.join("server").join("abuse.log")).unwrap();
    for why in ["already used", "another server", "expired", "banned"] {
        assert!(log.contains(why), "{why}: {log}");
    }

    // A token spent before a restart is refused after it, though the new server has
    // forgotten its nonce. Secrets refused altogether with `secret_login = "none"`.
    let spent = token(&claims("dave", ahead(), 8));
    let dave = connect_with(addr, &cert, "dave", spent.clone())
        .await
        .unwrap();
    drop(dave);
    server.shutdown().unwrap();
    let cfg = ServerConfig {
        secret_login: SecretLogin::None,
        ..cfg
    };
    let server = RunningServer::start(&world, cfg, &dir).unwrap();
    let (addr, cert) = (server.addr, server.cert_der.clone());
    assert_eq!(server.shared.tokens.as_ref().unwrap().nonces(), 0);
    let e = refused(addr, &cert, "dave", spent).await;
    assert!(e.contains("before the server started"), "{e}");
    let e = refused(addr, &cert, "carol", Auth::Secret("carol-secret".into())).await;
    assert!(e.contains("no secret logins"), "{e}");
    let fresh = Auth::Token(site.sign(&claims("dave", unix_ms() + SKEW_MS, 9)).unwrap());
    let dave = connect_with(addr, &cert, "dave", fresh).await.unwrap();
    drop(dave);
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
