//! The flow every host-owned login shares, whatever the provider: load the
//! stored credential, decide whether it is due for a refresh, refresh it
//! single-flight, write the rotated login back, keep a rotated login whose
//! write failed until a later write succeeds, and remember a refused refresh
//! as "sign in again" until the login changes. A [`LoginProvider`] supplies
//! only what really differs: where the login is stored, how it is read and
//! refreshed, and what a launch receives.
//!
//! Blocking throughout: the codex launch path is synchronous, and async
//! callers run [`HostLogin::credential`] on a blocking task, so the
//! single-flight lock is never held across an await.

use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;

pub mod claude;
pub(crate) mod codex;

/// Why a refresh produced no tokens. Neither variant carries a token or a
/// response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefreshFailure {
    /// The server refused the refresh token itself: only a new sign-in helps.
    Rejected,
    /// Anything else (transport, a 5xx, a 429, an answer without a token).
    Failed(String),
}

/// One lock per login key, created on first use: what makes the refreshes
/// of one login single-flight.
struct Flights {
    locks: Mutex<Option<HashMap<String, Arc<Mutex<()>>>>>,
}

impl Flights {
    const fn new() -> Self {
        Self {
            locks: Mutex::new(None),
        }
    }

    fn get(&self, key: &str) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .get_or_insert_with(HashMap::new)
            .entry(key.to_string())
            .or_default()
            .clone()
    }
}

/// A rotated login a store refused to take, and its place, type-erased so
/// one registry holds every provider's.
type KeptLogin = (Value, Arc<dyn Any + Send + Sync>);

/// Rotated logins a store refused to take, kept in memory by login key: the
/// old refresh token is already spent, so a kept one is that login's only
/// valid credential until a save succeeds, or until the store changes under
/// it (a new sign-in), which drops it.
struct Kept {
    pairs: Mutex<Option<KeptMap>>,
}

/// Login key → the kept login and the store's stamp when it was kept.
type KeptMap = HashMap<String, (KeptLogin, Option<String>)>;

impl Kept {
    const fn new() -> Self {
        Self {
            pairs: Mutex::new(None),
        }
    }

    /// Keep `value` for `key`, with the store's stamp as the failed save
    /// left it.
    fn keep(&self, key: &str, value: KeptLogin, stamp: Option<String>) {
        self.pairs
            .lock()
            .get_or_insert_with(HashMap::new)
            .insert(key.to_string(), (value, stamp));
    }

    /// The login kept for `key` while the store still has the stamp it had
    /// then; one kept under another stamp is dropped.
    fn current(&self, key: &str, stamp: Option<&str>) -> Option<KeptLogin> {
        let mut pairs = self.pairs.lock();
        let map = pairs.get_or_insert_with(HashMap::new);
        match map.get(key) {
            Some((value, at)) if at.as_deref() == stamp => Some(value.clone()),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    fn forget(&self, key: &str) {
        if let Some(map) = self.pairs.lock().as_mut() {
            map.remove(key);
        }
    }
}

/// Run one sign-in request to completion from blocking code: on its own
/// thread with its own runtime, so a caller that is itself on an async
/// worker never nests runtimes.
pub(crate) fn run_request<T: Send + 'static>(
    request: impl Future<Output = Result<T, RefreshFailure>> + Send + 'static,
) -> Result<T, RefreshFailure> {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| RefreshFailure::Failed(format!("no runtime: {e}")))?
            .block_on(request)
    })
    .join()
    .unwrap_or_else(|_| {
        Err(RefreshFailure::Failed(
            "the sign-in request panicked".into(),
        ))
    })
}

/// What the engine needs to know of a stored login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Creds {
    /// `None` when the login can't be refreshed.
    pub refresh: Option<String>,
    /// When the access token lapses, if that can be read.
    pub expires_at_ms: Option<i64>,
}

/// One provider's logins. Every method that touches a store is blocking.
pub(crate) trait LoginProvider: Send + Sync {
    /// Where a login was read from, and so where it is written back.
    type Place: Clone + Send + Sync + 'static;
    /// A refresh answer, as the provider parsed it.
    type Grant;
    /// What a launch receives.
    type Launch;
    /// Names the provider in the registry keys and the logs.
    const PROVIDER: &'static str;

    /// One key per login: its single-flight lock and registry entries.
    fn key(&self) -> String;
    /// A value that changes whenever the stored login does, without reading
    /// a secret. `None` = nothing stored.
    fn stamp(&self) -> Option<String>;
    /// The stored login as its raw text, and where it came from.
    fn load(&self) -> Result<Option<(String, Self::Place)>, String>;
    /// `None` when the text holds no login.
    fn parse(&self, json: &Value) -> Option<Creds>;
    /// Whether the login should be refreshed before a launch at `now_ms`.
    fn due(&self, creds: &Creds, json: &Value, now_ms: i64) -> bool;
    /// Whether a login of `len` bytes can be written back to `place` once
    /// refreshed. Checked before the refresh token is spent.
    fn fits(&self, _place: &Self::Place, _len: usize) -> bool {
        true
    }
    fn refresh(&self, refresh_token: &str) -> Result<Self::Grant, RefreshFailure>;
    /// Fold a refresh answer into the login; `false` when it can't be.
    fn apply(&self, json: &mut Value, grant: &Self::Grant, now_ms: i64) -> bool;
    fn save(&self, place: &Self::Place, json: &Value) -> Result<(), String>;
    fn launch(&self, json: &Value, creds: &Creds) -> Self::Launch;
    /// What a refusal is recorded against, for the login as `stamp` and
    /// `json` show it: it is revoked while this stays the same.
    fn mark_of(&self, stamp: Option<&str>, json: &Value) -> Option<String>;
    /// [`mark_of`](Self::mark_of) for the login as stored now, for a probe.
    fn current_mark(&self) -> Option<String>;
    /// Where a refusal is recorded to outlive the app; `None` keeps it in
    /// memory only.
    fn mark_path(&self) -> Option<PathBuf> {
        None
    }
    /// The clock the margin and expiry are judged by.
    fn now_ms(&self) -> i64 {
        now_ms()
    }
}

/// Why no credential could be had. Never carries a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoginError {
    SignedOut,
    /// The refresh token was refused; only a new sign-in helps.
    Revoked,
    /// The token is past its expiry (or a forced refresh was asked for) and
    /// the refresh could not be completed. The stored login is untouched.
    Unavailable(String),
}

/// What the credential is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Demand {
    /// A launch: refresh when due, else use the stored login.
    Launch,
    /// Replace a token the API rejected although it wasn't due: the store's
    /// newer one if it is past `rejected_expires_at_ms`, else a forced refresh.
    Replace { rejected_expires_at_ms: i64 },
}

static FLIGHTS: Flights = Flights::new();
static KEPT: Kept = Kept::new();
static REVOKED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// The hex SHA-256 of `bytes`: how a mark names a login key, or records a
/// refused token, without holding either.
pub(crate) fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Where a refused login's mark is kept across restarts:
/// `<accounts root>/.state/<dirname>/<first 16 hex of the key's digest>`.
pub(crate) fn mark_file(dirname: &str, key: &str) -> Option<PathBuf> {
    // A test that doesn't point the accounts root at a tempdir must not write
    // into the developer's real one; its marks stay in memory.
    if cfg!(test) && std::env::var_os(crate::agent::accounts::ACCOUNTS_ROOT_ENV).is_none() {
        return None;
    }
    let root = crate::agent::accounts::accounts_root().ok()?;
    Some(
        root.join(".state")
            .join(dirname)
            .join(&digest(key.as_bytes())[..16]),
    )
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The engine over one provider's login.
pub(crate) struct HostLogin<P: LoginProvider> {
    provider: P,
}

/// The login being worked from.
struct Current<Place> {
    /// The store's stamp; `None` for a kept login, which is newer than any
    /// refusal on record.
    stamp: Option<String>,
    raw: String,
    json: Value,
    place: Place,
}

impl<P: LoginProvider> HostLogin<P> {
    pub(crate) fn new(provider: P) -> Self {
        Self { provider }
    }

    #[cfg(test)]
    pub(crate) fn provider(&self) -> &P {
        &self.provider
    }

    fn key(&self) -> String {
        format!("{}:{}", P::PROVIDER, self.provider.key())
    }

    /// The credential a launch (or a replacement) gets: the stored login,
    /// refreshed first when it is due. A refresh that can't be made, or that
    /// fails for a reason other than a refused token, still yields the stored
    /// login to a launch while its access token is unexpired.
    pub(crate) fn credential(&self, demand: Demand) -> Result<P::Launch, LoginError> {
        let key = self.key();
        let lock = FLIGHTS.get(&key);
        // Held across the refresh and the write-back: whoever waits here reads
        // the rotated login, never the one this flight just spent.
        let _flight = lock.lock();

        let Some(mut cur) = self.current_login(&key)? else {
            return Err(LoginError::SignedOut);
        };
        if cur.stamp.is_some() && self.revoked_at(&key, cur.stamp.as_deref(), &cur.json) {
            return Err(LoginError::Revoked);
        }
        let mut creds = self
            .provider
            .parse(&cur.json)
            .ok_or(LoginError::SignedOut)?;
        let mut rechecked = false;
        loop {
            let now = self.provider.now_ms();
            let due = match demand {
                Demand::Launch => self.provider.due(&creds, &cur.json, now),
                Demand::Replace {
                    rejected_expires_at_ms,
                } => {
                    creds
                        .expires_at_ms
                        .map_or(true, |e| e <= rejected_expires_at_ms)
                        || self.provider.due(&creds, &cur.json, now)
                }
            };
            if !due {
                return Ok(self.provider.launch(&cur.json, &creds));
            }
            let Some(refresh) = creds.refresh.clone() else {
                return if matches!(demand, Demand::Launch) && unexpired(&creds, now) {
                    Ok(self.provider.launch(&cur.json, &creds))
                } else {
                    Err(LoginError::Revoked)
                };
            };
            // Spending the refresh token is only safe when its successor can
            // be stored.
            if !self.provider.fits(&cur.place, cur.raw.len()) {
                tracing::warn!(
                    provider = P::PROVIDER,
                    "login too large to store back after a refresh; not refreshing"
                );
                return self.fall_back(
                    demand,
                    &cur.json,
                    &creds,
                    "the stored login is too large to write back after a refresh".into(),
                );
            }
            match self.provider.refresh(&refresh) {
                Ok(grant) => {
                    let mut json = cur.json.clone();
                    if !self
                        .provider
                        .apply(&mut json, &grant, self.provider.now_ms())
                    {
                        return Err(LoginError::SignedOut);
                    }
                    // Saved (or kept) before anything else can fail: the old
                    // refresh token is already spent.
                    self.save_rotated(&key, &cur.place, &json);
                    let creds = self.provider.parse(&json).ok_or(LoginError::SignedOut)?;
                    tracing::info!(provider = P::PROVIDER, "login refreshed");
                    return Ok(self.provider.launch(&json, &creds));
                }
                Err(RefreshFailure::Rejected) => {
                    // Another process (the user's own CLI, on the default
                    // login) may have rotated the login while this request
                    // was out; its new login is in the store and still good.
                    let fresh = self.read_store()?;
                    let fresh_creds = fresh.as_ref().and_then(|f| self.provider.parse(&f.json));
                    let moved = fresh_creds
                        .as_ref()
                        .is_some_and(|c| c.refresh.as_deref() != Some(refresh.as_str()));
                    // A login rotated meanwhile (a refresh token gone counts)
                    // is taken as freshly read: launched on unless it is due,
                    // else refreshed once more.
                    if moved && !rechecked {
                        rechecked = true;
                        cur = fresh.expect("parsed from it");
                        creds = fresh_creds.expect("checked above");
                        continue;
                    }
                    // Marked only when the store still holds the token that
                    // was refused, or nothing at all: a login that moved on
                    // again is someone else's newer one, and anything else
                    // there is not for this refusal to judge.
                    let still_refused = fresh_creds
                        .as_ref()
                        .is_some_and(|c| c.refresh.as_deref() == Some(refresh.as_str()));
                    if still_refused || fresh.is_none() {
                        let (stamp, json) = match &fresh {
                            Some(f) => (f.stamp.clone().or(cur.stamp.clone()), &f.json),
                            None => (cur.stamp.clone(), &cur.json),
                        };
                        self.record_revoked(&key, stamp.as_deref(), json);
                    }
                    tracing::warn!(
                        provider = P::PROVIDER,
                        "login refresh refused; the account must sign in again"
                    );
                    return Err(LoginError::Revoked);
                }
                Err(RefreshFailure::Failed(reason)) => {
                    tracing::warn!(provider = P::PROVIDER, %reason, "login refresh failed");
                    return self.fall_back(demand, &cur.json, &creds, reason);
                }
            }
        }
    }

    /// Whether a refusal is on record for this login and it hasn't changed
    /// since — Settings' "sign in again". Free unless a refusal is on record.
    pub(crate) fn is_revoked(&self) -> bool {
        let key = self.key();
        let Some(recorded) = self.recorded_mark(&key) else {
            return false;
        };
        if self.provider.current_mark().as_deref() == Some(recorded.as_str()) {
            return true;
        }
        self.clear_mark(&key);
        false
    }

    fn fall_back(
        &self,
        demand: Demand,
        json: &Value,
        creds: &Creds,
        reason: String,
    ) -> Result<P::Launch, LoginError> {
        if matches!(demand, Demand::Launch) && unexpired(creds, self.provider.now_ms()) {
            Ok(self.provider.launch(json, creds))
        } else {
            Err(LoginError::Unavailable(reason))
        }
    }

    fn read_store(&self) -> Result<Option<Current<P::Place>>, LoginError> {
        let stamp = self.provider.stamp();
        let loaded = self.provider.load().map_err(LoginError::Unavailable)?;
        Ok(loaded.and_then(|(raw, place)| {
            let json = serde_json::from_str(&raw).ok()?;
            Some(Current {
                stamp: stamp.clone(),
                raw,
                json,
                place,
            })
        }))
    }

    /// A kept login written now if the store takes it, else used as it is; a
    /// kept login whose store changed since (a new sign-in) is dropped.
    /// Otherwise what the store holds.
    fn current_login(&self, key: &str) -> Result<Option<Current<P::Place>>, LoginError> {
        let stamp = self.provider.stamp();
        let Some((json, place)) = KEPT.current(key, stamp.as_deref()) else {
            return self.read_store();
        };
        let Some(place) = place.downcast_ref::<P::Place>().cloned() else {
            KEPT.forget(key);
            return self.read_store();
        };
        match self.provider.save(&place, &json) {
            Ok(()) => {
                KEPT.forget(key);
                tracing::info!(
                    provider = P::PROVIDER,
                    "stored a refreshed login that an earlier write refused"
                );
                self.read_store()
            }
            Err(e) => {
                tracing::error!(provider = P::PROVIDER, error = %e, "refreshed login still can't be stored");
                Ok(Some(Current {
                    stamp: None,
                    raw: json.to_string(),
                    json,
                    place,
                }))
            }
        }
    }

    /// Store a rotated login, or keep it when the store refuses: the old
    /// refresh token is spent, so it is the account's only valid one.
    fn save_rotated(&self, key: &str, place: &P::Place, json: &Value) {
        match self.provider.save(place, json) {
            Ok(()) => KEPT.forget(key),
            Err(e) => {
                tracing::error!(provider = P::PROVIDER, error = %e, "refreshed login could not be stored; keeping it to retry");
                KEPT.keep(
                    key,
                    (json.clone(), Arc::new(place.clone())),
                    self.provider.stamp(),
                );
            }
        }
    }

    /// The mark on record: this process's first, else the one a past run
    /// left in the provider's mark file. An empty file (one being written)
    /// is no mark.
    fn recorded_mark(&self, key: &str) -> Option<String> {
        let remembered = REVOKED.lock().as_ref().and_then(|m| m.get(key).cloned());
        if remembered.is_some() {
            return remembered;
        }
        let path = self.provider.mark_path()?;
        std::fs::read_to_string(path)
            .ok()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
    }

    fn clear_mark(&self, key: &str) {
        if let Some(path) = self.provider.mark_path() {
            let _ = std::fs::remove_file(path);
        }
        if let Some(map) = REVOKED.lock().as_mut() {
            map.remove(key);
        }
    }

    /// Whether the login as `stamp`/`json` show it is the one a refusal was
    /// recorded against; a changed login clears the record.
    fn revoked_at(&self, key: &str, stamp: Option<&str>, json: &Value) -> bool {
        let Some(recorded) = self.recorded_mark(key) else {
            return false;
        };
        if self.provider.mark_of(stamp, json).as_deref() == Some(recorded.as_str()) {
            return true;
        }
        self.clear_mark(key);
        false
    }

    fn record_revoked(&self, key: &str, stamp: Option<&str>, json: &Value) {
        let Some(mark) = self.provider.mark_of(stamp, json) else {
            return;
        };
        if let Some(path) = self.provider.mark_path() {
            let write = || -> std::io::Result<()> {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                crate::agent::credential_file::write_private_file(&path, mark.as_bytes())
            };
            if let Err(e) = write() {
                tracing::warn!(provider = P::PROVIDER, error = %e, "could not record the login as revoked past a restart");
            }
        }
        REVOKED
            .lock()
            .get_or_insert_with(HashMap::new)
            .insert(key.to_string(), mark);
    }
}

/// Still usable at `now_ms`: unexpired, or of unknown expiry.
fn unexpired(creds: &Creds, now_ms: i64) -> bool {
    creds.expires_at_ms.map_or(true, |e| e > now_ms)
}

#[cfg(test)]
mod tests;
