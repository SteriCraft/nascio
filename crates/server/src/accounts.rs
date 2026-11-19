//! Player accounts (S7): a name and a secret, kept as an Argon2id hash with its own salt in
//! `<data>/server/accounts.toml` (rewritten durably on each change).
//!
//! Hashing takes tens of milliseconds by design: the server runs it on a blocking thread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use nascio_voxel::delta::write_durable;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use crate::spawn::AccountSpawn;

/// One account as stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    /// Salt and hash, hexadecimal.
    pub salt: String,
    pub hash: String,
    /// Argon2id parameters used for this hash: memory (KiB), passes, lanes.
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
    /// Creation time, ms since the Unix epoch.
    pub created_unix_ms: i64,
    /// An administrator (may log in with the secret when `secret_login = "admin"`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub admin: bool,
    /// The account is banned (an expired ban is kept but ignored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ban: Option<Ban>,
    /// Where the player comes back at each login, drawn at the first one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<AccountSpawn>,
    /// Where the player was when it left (feet, metres, exact); `None`: never left alive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<[f64; 3]>,
}

/// A ban of an account, temporary or permanent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ban {
    /// When it was decided, ms since the Unix epoch.
    pub since_unix_ms: i64,
    /// When it ends, ms since the Unix epoch; `None`: permanent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_unix_ms: Option<i64>,
    pub reason: String,
}

impl Ban {
    /// Still in force at `now`.
    pub fn active(&self, now_unix_ms: i64) -> bool {
        self.until_unix_ms.is_none_or(|u| now_unix_ms < u)
    }

    /// What the player is told.
    pub fn message(&self, now_unix_ms: i64) -> String {
        let until = match self.until_unix_ms {
            None => "permanently".to_owned(),
            Some(u) => format!("for {}", format_duration(u - now_unix_ms)),
        };
        if self.reason.is_empty() {
            format!("banned {until}")
        } else {
            format!("banned {until}: {}", self.reason)
        }
    }
}

/// `30s`, `30m`, `12h`, `7d` or `2w` in milliseconds.
pub fn parse_duration(s: &str) -> Option<i64> {
    let unit = match s.chars().last()? {
        's' => 1_000,
        'm' => 60_000,
        'h' => 3_600_000,
        'd' => 86_400_000,
        'w' => 7 * 86_400_000,
        _ => return None,
    };
    let n: i64 = s[..s.len() - 1].parse().ok()?;
    (n > 0).then(|| n.checked_mul(unit)).flatten()
}

/// A duration for a person: the largest whole unit, rounded up.
pub fn format_duration(ms: i64) -> String {
    let ms = ms.max(0);
    for (unit, name) in [(86_400_000, "d"), (3_600_000, "h"), (60_000, "min")] {
        if ms >= unit {
            return format!("{} {name}", (ms + unit - 1) / unit);
        }
    }
    format!("{} s", (ms + 999) / 1_000)
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default, rename = "account")]
    accounts: Vec<Account>,
}

/// What a login attempt gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Login {
    /// Known name, right secret (or a valid token).
    Ok,
    /// Unknown name: the account was created (open registration).
    Created,
}

/// The accounts of the server.
pub struct Accounts {
    path: PathBuf,
    accounts: Mutex<BTreeMap<String, Account>>,
    /// Hashing parameters of new accounts.
    params: Params,
    /// Held while a snapshot is taken and written: the last write is always the newest.
    writing: Mutex<()>,
}

/// A change the simulation hands to the [`writer`].
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// The player's last position (`None`: it died, it comes back at its point).
    Last(String, Option<[f64; 3]>),
    /// A new spawn point (the former one was blocked).
    Spawn(String, AccountSpawn),
}

/// A name: 3 to 16 characters among ASCII letters, digits and `_`.
pub fn check_name(name: &str) -> anyhow::Result<()> {
    ensure!(
        (3..=16).contains(&name.len())
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "a name has 3 to 16 characters among letters, digits and _"
    );
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> anyhow::Result<Vec<u8>> {
    ensure!(s.len().is_multiple_of(2), "odd hex length");
    (0..s.len())
        .step_by(2)
        .map(|i| Ok(u8::from_str_radix(&s[i..i + 2], 16)?))
        .collect()
}

fn hash(params: &Params, secret: &str, salt: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = vec![0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params.clone())
        .hash_password_into(secret.as_bytes(), salt, &mut out)
        .map_err(|e| anyhow::anyhow!("argon2: {e}"))?;
    Ok(out)
}

/// Constant-time comparison (no early exit on the first difference).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Accounts {
    /// Opens (or starts) the accounts of `dir/accounts.toml`.
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        Self::open_with(dir, Params::default())
    }

    /// With other hashing parameters for new accounts (tests use cheap ones).
    pub fn open_with(dir: &Path, params: Params) -> anyhow::Result<Self> {
        let path = dir.join("accounts.toml");
        let file: File = if path.exists() {
            toml::from_str(&std::fs::read_to_string(&path)?)
                .with_context(|| format!("reading {}", path.display()))?
        } else {
            File::default()
        };
        let accounts = file
            .accounts
            .into_iter()
            .map(|a| (a.name.clone(), a))
            .collect();
        Ok(Self {
            path,
            accounts: Mutex::new(accounts),
            params,
            writing: Mutex::new(()),
        })
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    pub fn exists(&self, name: &str) -> bool {
        self.lock().contains_key(name)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Account>> {
        self.accounts.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes the accounts as they are now (durably). The accounts stay readable meanwhile:
    /// only the snapshot is taken under their lock.
    pub fn save(&self) -> anyhow::Result<()> {
        let _w = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        let file = File {
            accounts: self.lock().values().cloned().collect(),
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_durable(&self.path, toml::to_string(&file)?.as_bytes())
    }

    /// Applies changes in memory (written by the next [`Self::save`]); unknown accounts are
    /// skipped. Whether anything changed.
    pub fn apply(&self, changes: &[Change]) -> bool {
        let mut accounts = self.lock();
        let mut any = false;
        for c in changes {
            match c {
                Change::Last(name, pos) => {
                    if let Some(a) = accounts.get_mut(name)
                        && a.last != *pos
                    {
                        a.last = *pos;
                        any = true;
                    }
                }
                Change::Spawn(name, s) => {
                    if let Some(a) = accounts.get_mut(name) {
                        a.spawn = Some(*s);
                        any = true;
                    }
                }
            }
        }
        any
    }

    /// The last position of `name`, if it left alive.
    pub fn last(&self, name: &str) -> Option<[f64; 3]> {
        self.lock().get(name).and_then(|a| a.last)
    }

    /// Creates an account (refused if the name is taken or invalid, or the secret too
    /// short).
    pub fn create(&self, name: &str, secret: &str, now_unix_ms: i64) -> anyhow::Result<()> {
        check_name(name)?;
        ensure!(secret.len() >= 8, "a secret has at least 8 characters");
        let mut salt = [0u8; 16];
        SystemRandom::new()
            .fill(&mut salt)
            .map_err(|_| anyhow::anyhow!("no system randomness"))?;
        let h = hash(&self.params, secret, &salt)?;
        let mut accounts = self.lock();
        if accounts.contains_key(name) {
            bail!("the name {name} is taken");
        }
        accounts.insert(
            name.to_owned(),
            Account {
                name: name.to_owned(),
                salt: hex(&salt),
                hash: hex(&h),
                m_kib: self.params.m_cost(),
                t: self.params.t_cost(),
                p: self.params.p_cost(),
                created_unix_ms: now_unix_ms,
                admin: false,
                ban: None,
                spawn: None,
                last: None,
            },
        );
        drop(accounts);
        self.save()
    }

    /// Checks a login; with `register`, an unknown name creates the account.
    pub fn login(
        &self,
        name: &str,
        secret: &str,
        register: bool,
        now_unix_ms: i64,
    ) -> anyhow::Result<Login> {
        check_name(name)?;
        let account = self.lock().get(name).cloned();
        let Some(a) = account else {
            ensure!(register, "unknown account");
            self.create(name, secret, now_unix_ms)?;
            return Ok(Login::Created);
        };
        // An account made from a token has no secret.
        ensure!(!a.hash.is_empty(), "wrong secret");
        let params = Params::new(a.m_kib, a.t, a.p, Some(32))
            .map_err(|e| anyhow::anyhow!("argon2 parameters: {e}"))?;
        let h = hash(&params, secret, &unhex(&a.salt)?)?;
        ensure!(same(&h, &unhex(&a.hash)?), "wrong secret");
        Ok(Login::Ok)
    }

    /// After a valid token: creates the account (without a secret) if it does not exist.
    pub fn token_login(&self, name: &str, now_unix_ms: i64) -> anyhow::Result<Login> {
        check_name(name)?;
        let mut accounts = self.lock();
        if accounts.contains_key(name) {
            return Ok(Login::Ok);
        }
        accounts.insert(
            name.to_owned(),
            Account {
                name: name.to_owned(),
                salt: String::new(),
                hash: String::new(),
                m_kib: 0,
                t: 0,
                p: 0,
                created_unix_ms: now_unix_ms,
                admin: false,
                ban: None,
                spawn: None,
                last: None,
            },
        );
        drop(accounts);
        self.save()?;
        Ok(Login::Created)
    }

    /// The ban in force on `name` at `now`, if any.
    pub fn banned(&self, name: &str, now_unix_ms: i64) -> Option<Ban> {
        self.lock()
            .get(name)?
            .ban
            .clone()
            .filter(|b| b.active(now_unix_ms))
    }

    /// Bans an account until `until` (`None`: permanently); replaces a former ban.
    pub fn ban(
        &self,
        name: &str,
        until_unix_ms: Option<i64>,
        reason: &str,
        now_unix_ms: i64,
    ) -> anyhow::Result<Ban> {
        let mut accounts = self.lock();
        let Some(a) = accounts.get_mut(name) else {
            bail!("unknown account {name}");
        };
        let ban = Ban {
            since_unix_ms: now_unix_ms,
            until_unix_ms,
            reason: reason.to_owned(),
        };
        a.ban = Some(ban.clone());
        drop(accounts);
        self.save()?;
        Ok(ban)
    }

    /// Lifts the ban of an account; false if it was not banned at `now`.
    pub fn unban(&self, name: &str, now_unix_ms: i64) -> anyhow::Result<bool> {
        let mut accounts = self.lock();
        let Some(a) = accounts.get_mut(name) else {
            bail!("unknown account {name}");
        };
        let was = a.ban.take().is_some_and(|b| b.active(now_unix_ms));
        drop(accounts);
        self.save()?;
        Ok(was)
    }

    /// The bans in force at `now`, by name.
    pub fn bans(&self, now_unix_ms: i64) -> Vec<(String, Ban)> {
        self.lock()
            .values()
            .filter_map(|a| {
                let b = a.ban.as_ref().filter(|b| b.active(now_unix_ms))?;
                Some((a.name.clone(), b.clone()))
            })
            .collect()
    }

    /// Every account as (name, is administrator), sorted by name.
    pub fn users(&self) -> Vec<(String, bool)> {
        let mut users: Vec<(String, bool)> = self
            .lock()
            .values()
            .map(|a| (a.name.clone(), a.admin))
            .collect();
        users.sort();
        users
    }

    /// The spawn point of `name`, if drawn.
    pub fn spawn(&self, name: &str) -> Option<AccountSpawn> {
        self.lock().get(name).and_then(|a| a.spawn)
    }

    /// Keeps the spawn point of `name` (unknown accounts are refused).
    pub fn set_spawn(&self, name: &str, spawn: AccountSpawn) -> anyhow::Result<()> {
        let mut accounts = self.lock();
        let Some(a) = accounts.get_mut(name) else {
            bail!("no account {name}");
        };
        a.spawn = Some(spawn);
        drop(accounts);
        self.save()
    }

    /// Accounts per spawn site (`n` sites).
    pub fn per_site(&self, n: usize) -> Vec<usize> {
        let mut count = vec![0; n];
        for a in self.lock().values() {
            if let Some(c) = a.spawn.and_then(|s| count.get_mut(s.site as usize)) {
                *c += 1;
            }
        }
        count
    }

    pub fn is_admin(&self, name: &str) -> bool {
        self.lock().get(name).is_some_and(|a| a.admin)
    }

    /// Makes an account an administrator or a player.
    pub fn set_admin(&self, name: &str, admin: bool) -> anyhow::Result<()> {
        let mut accounts = self.lock();
        let Some(a) = accounts.get_mut(name) else {
            bail!("unknown account {name}");
        };
        a.admin = admin;
        drop(accounts);
        self.save()
    }
}

/// The writer of the simulation's changes (last positions, new spawn points): applied in
/// memory as they come, so that a login reads them at once, and written at most once every
/// `min_interval` (`write_durable` off the tick). Returns once every sender is gone, after a
/// last write.
pub fn writer(accounts: Arc<Accounts>, rx: Receiver<Vec<Change>>, min_interval: Duration) {
    let mut dirty = false;
    let mut written: Option<Instant> = None;
    loop {
        let due = written.map_or(Duration::ZERO, |w| {
            (w + min_interval).saturating_duration_since(Instant::now())
        });
        let wait = if dirty {
            due
        } else {
            Duration::from_secs(3_600)
        };
        match rx.recv_timeout(wait) {
            Ok(changes) => {
                dirty |= accounts.apply(&changes);
                while let Ok(more) = rx.try_recv() {
                    dirty |= accounts.apply(&more);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if dirty && let Err(e) = accounts.save() {
                    crate::say_err!("accounts: last positions not written: {e:#}");
                }
                return;
            }
        }
        if dirty && written.is_none_or(|w| w.elapsed() >= min_interval) {
            if let Err(e) = accounts.save() {
                crate::say_err!("accounts: last positions not written: {e:#}");
            }
            dirty = false;
            written = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cheap() -> Params {
        Params::new(256, 1, 1, Some(32)).unwrap()
    }

    #[test]
    fn accounts_are_created_checked_and_kept() {
        let dir = std::env::temp_dir().join(format!("nascio-accounts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Accounts::open_with(&dir, cheap()).unwrap();
        assert_eq!(
            a.login("alice", "correct horse", true, 1).unwrap(),
            Login::Created
        );
        assert_eq!(
            a.login("alice", "correct horse", true, 2).unwrap(),
            Login::Ok
        );
        assert!(a.login("alice", "wrong horse!", true, 3).is_err());
        assert!(
            a.login("bob", "whatever1", false, 4).is_err(),
            "closed registration"
        );
        assert!(a.login("x", "whatever1", true, 5).is_err(), "short name");
        assert!(a.login("bad name", "whatever1", true, 5).is_err());
        assert!(a.login("carol", "short", true, 5).is_err(), "short secret");
        assert!(a.create("alice", "another one", 6).is_err(), "taken");
        // Reopened: the account is there, its hash is not the secret.
        let b = Accounts::open_with(&dir, cheap()).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(
            b.login("alice", "correct horse", false, 7).unwrap(),
            Login::Ok
        );
        let text = std::fs::read_to_string(dir.join("accounts.toml")).unwrap();
        assert!(!text.contains("correct horse"));
        assert!(!text.contains("ban") && !text.contains("admin"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bans_are_kept_and_expire() {
        let dir = std::env::temp_dir().join(format!("nascio-bans-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Accounts::open_with(&dir, cheap()).unwrap();
        a.create("alice", "correct horse", 0).unwrap();
        a.create("bob", "battery staple", 0).unwrap();
        assert!(a.ban("carol", None, "", 0).is_err(), "unknown");
        a.ban("alice", Some(1_000), "spam", 0).unwrap();
        a.ban("bob", None, "", 0).unwrap();
        a.set_admin("bob", true).unwrap();
        // Reopened: the bans and the admin flag are there.
        let b = Accounts::open_with(&dir, cheap()).unwrap();
        assert_eq!(b.bans(500).len(), 2);
        assert_eq!(b.banned("alice", 999).unwrap().reason, "spam");
        assert!(b.banned("alice", 1_000).is_none(), "expired");
        assert_eq!(b.bans(1_000).len(), 1);
        assert!(b.is_admin("bob") && !b.is_admin("alice"));
        assert!(b.banned("bob", i64::MAX).is_some(), "permanent");
        assert!(b.unban("bob", 2_000).unwrap());
        assert!(!b.unban("alice", 2_000).unwrap(), "already expired");
        assert!(b.bans(0).is_empty());
        // An account made from a token has no secret: no secret opens it.
        assert_eq!(b.token_login("dave", 3).unwrap(), Login::Created);
        assert_eq!(b.token_login("dave", 4).unwrap(), Login::Ok);
        assert_eq!(b.token_login("alice", 4).unwrap(), Login::Ok);
        assert!(b.login("dave", "", false, 5).is_err());
        assert!(b.login("dave", "anything at all", false, 5).is_err());
        assert!(Accounts::open_with(&dir, cheap()).unwrap().exists("dave"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30m"), Some(1_800_000));
        assert_eq!(parse_duration("7d"), Some(604_800_000));
        assert_eq!(parse_duration("2w"), Some(1_209_600_000));
        for bad in ["", "m", "0m", "-3h", "3x", "1.5h", "spam"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
        assert_eq!(format_duration(90_000), "2 min");
        assert_eq!(format_duration(86_400_000), "1 d");
        assert_eq!(format_duration(1), "1 s");
    }

    fn stats(label: &str, mut ms: Vec<f64>) {
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "{label}: min {:.3} ms, median {:.3} ms, max {:.3} ms",
            ms[0],
            ms[ms.len() / 2],
            ms[ms.len() - 1]
        );
    }

    #[test]
    #[ignore]
    fn write_cost_500_accounts() {
        let dir = std::env::temp_dir().join(format!("nascio-accounts-cost-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Accounts::open_with(&dir, cheap()).unwrap();
        for i in 0..500usize {
            let name = format!("player_{i:03}");
            a.token_login(&name, 0).unwrap();
            a.apply(&[
                Change::Spawn(
                    name.clone(),
                    AccountSpawn {
                        site: (i % 10) as u32,
                        x: 12_345 + i as i32,
                        z: 67_890 - i as i32,
                    },
                ),
                Change::Last(
                    name,
                    Some([
                        12_345.678_901_234_5 + i as f64 * 0.1,
                        87.123_456_789,
                        67_890.987_654_321,
                    ]),
                ),
            ]);
        }
        let mut write = Vec::new();
        for _ in 0..20 {
            let t = Instant::now();
            a.save().unwrap();
            write.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let size = std::fs::metadata(dir.join("accounts.toml")).unwrap().len();
        println!("accounts.toml: {size} bytes");
        stats("full save", write);
        let mut ser = Vec::new();
        for _ in 0..20 {
            let t = Instant::now();
            let s = toml::to_string(&File {
                accounts: a.lock().values().cloned().collect(),
            })
            .unwrap();
            ser.push(t.elapsed().as_secs_f64() * 1e3);
            std::hint::black_box(s);
        }
        stats("serialization only", ser);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
