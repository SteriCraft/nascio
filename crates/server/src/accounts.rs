//! Player accounts (S7): a name and a secret, kept as an Argon2id hash with its own salt in
//! `<data>/server/accounts.toml` (rewritten durably on each change).
//!
//! Hashing takes tens of milliseconds by design: the server runs it on a blocking thread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, bail, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use nascio_voxel::delta::write_durable;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

/// One account as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default, rename = "account")]
    accounts: Vec<Account>,
}

/// What a login attempt gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Login {
    /// Known name, right secret.
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

    fn save(&self, accounts: &BTreeMap<String, Account>) -> anyhow::Result<()> {
        let file = File {
            accounts: accounts.values().cloned().collect(),
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_durable(&self.path, toml::to_string(&file)?.as_bytes())
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
            },
        );
        self.save(&accounts)
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
        let params = Params::new(a.m_kib, a.t, a.p, Some(32))
            .map_err(|e| anyhow::anyhow!("argon2 parameters: {e}"))?;
        let h = hash(&params, secret, &unhex(&a.salt)?)?;
        ensure!(same(&h, &unhex(&a.hash)?), "wrong secret");
        Ok(Login::Ok)
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}
