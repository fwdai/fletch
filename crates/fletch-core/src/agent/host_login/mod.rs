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

// No provider runs on the engine until the claude and codex adapters move
// onto it; the lint comes back with them.
#![cfg_attr(not(test), allow(dead_code))]

use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;

/// Why a refresh produced no tokens. Neither variant carries a token or a
/// response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefreshFailure {
    /// The server refused the refresh token itself: only a new sign-in helps.
    Rejected,
    /// Anything else (transport, a 5xx, a 429, an answer without a token).
    Failed(String),
}

/// One lock per key, created on first use: what makes the refreshes of one
/// login single-flight. `T` is the lock kind the caller holds across its work
/// (an async mutex where the refresh awaits, a blocking one where it doesn't).
pub(crate) struct Flights<T> {
    locks: Mutex<Option<HashMap<String, Arc<T>>>>,
}

impl<T: Default> Flights<T> {
    pub(crate) const fn new() -> Self {
        Self {
            locks: Mutex::new(None),
        }
    }

    pub(crate) fn get(&self, key: &str) -> Arc<T> {
        self.locks
            .lock()
            .get_or_insert_with(HashMap::new)
            .entry(key.to_string())
            .or_default()
            .clone()
    }
}

/// Rotated logins a store refused to take, kept in memory by login key: the
/// old refresh token is already spent, so a kept one is that login's only
/// valid credential until a save succeeds, or until the store changes under
/// it (a new sign-in), which drops it.
pub(crate) struct Kept<T> {
    pairs: Mutex<Option<KeptMap<T>>>,
}

/// Login key → the kept value and the store's stamp when it was kept.
type KeptMap<T> = HashMap<String, (T, Option<String>)>;

impl<T: Clone> Kept<T> {
    pub(crate) const fn new() -> Self {
        Self {
            pairs: Mutex::new(None),
        }
    }

    /// Keep `value` for `key`, with the store's stamp as the failed save
    /// left it.
    pub(crate) fn keep(&self, key: &str, value: T, stamp: Option<String>) {
        self.pairs
            .lock()
            .get_or_insert_with(HashMap::new)
            .insert(key.to_string(), (value, stamp));
    }

    /// The value kept for `key` while the store still has the stamp it had
    /// then; one kept under another stamp is dropped.
    pub(crate) fn current(&self, key: &str, stamp: Option<&str>) -> Option<T> {
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

    #[cfg(test)]
    fn has(&self, key: &str) -> bool {
        self.pairs
            .lock()
            .as_ref()
            .is_some_and(|m| m.contains_key(key))
    }

    pub(crate) fn forget(&self, key: &str) {
        if let Some(map) = self.pairs.lock().as_mut() {
            map.remove(key);
        }
    }
}

/// What the engine needs to know of a stored login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Creds {
    /// `None` when the login can't be refreshed.
    pub refresh: Option<String>,
    /// When the access token lapses, if that can be read.
    pub expires_at_ms: Option<i64>,
}

/// What a refused refresh does when the stored refresh token turned out to
/// have changed while the request was out (another process rotated it).
/// Two rules until the providers converge on one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterRotation {
    /// Launch on the rotated login while its access token is unexpired;
    /// never refresh again.
    UseIfUnexpired,
    /// Treat the rotated login as freshly read: launch on it unless it is
    /// due, else refresh it once more.
    Recheck,
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
    const AFTER_ROTATION: AfterRotation;

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

/// A kept login and its place, the place type-erased so one registry holds
/// every provider's.
type KeptLogin = (Value, Arc<dyn Any + Send + Sync>);

static FLIGHTS: Flights<Mutex<()>> = Flights::new();
static KEPT: Kept<KeptLogin> = Kept::new();
static REVOKED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

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
            let now = now_ms();
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
                    if !self.provider.apply(&mut json, &grant, now_ms()) {
                        return Err(LoginError::SignedOut);
                    }
                    let creds = self.provider.parse(&json).ok_or(LoginError::SignedOut)?;
                    self.save_rotated(&key, &cur.place, &json);
                    tracing::info!(provider = P::PROVIDER, "login refreshed");
                    return Ok(self.provider.launch(&json, &creds));
                }
                Err(RefreshFailure::Rejected) => {
                    // Another process (the user's own CLI, on the default
                    // login) may have rotated the login while this request
                    // was out; its new login is in the store and still good.
                    let fresh = self.read_store()?;
                    let rotated = fresh.as_ref().and_then(|f| {
                        let creds = self.provider.parse(&f.json)?;
                        (creds.refresh.as_deref() != Some(refresh.as_str())).then_some(creds)
                    });
                    match (rotated, P::AFTER_ROTATION) {
                        (Some(rot), AfterRotation::UseIfUnexpired) if unexpired_strict(&rot) => {
                            let fresh = fresh.expect("rotated came from it");
                            return Ok(self.provider.launch(&fresh.json, &rot));
                        }
                        (Some(rot), AfterRotation::Recheck)
                            if !rechecked && rot.refresh.is_some() =>
                        {
                            rechecked = true;
                            cur = fresh.expect("rotated came from it");
                            creds = rot;
                            continue;
                        }
                        _ => {}
                    }
                    let (stamp, json) = match &fresh {
                        Some(f) => (f.stamp.clone().or(cur.stamp.clone()), &f.json),
                        None => (cur.stamp.clone(), &cur.json),
                    };
                    self.record_revoked(&key, stamp.as_deref(), json);
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
        if matches!(demand, Demand::Launch) && unexpired(creds, now_ms()) {
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

    fn recorded_mark(&self, key: &str) -> Option<String> {
        if let Some(path) = self.provider.mark_path() {
            return std::fs::read_to_string(path)
                .ok()
                .map(|m| m.trim().to_string());
        }
        REVOKED.lock().as_ref()?.get(key).cloned()
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
                std::fs::write(&path, &mark)
            };
            if let Err(e) = write() {
                tracing::warn!(provider = P::PROVIDER, error = %e, "could not record the login as revoked");
            }
            return;
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

/// Known to be unexpired now.
fn unexpired_strict(creds: &Creds) -> bool {
    creds.expires_at_ms.is_some_and(|e| e > now_ms())
}

pub mod claude;

/// Whether a rotated login is kept for the engine key `key` (`provider:key`).
#[cfg(test)]
pub(crate) fn is_kept(key: &str) -> bool {
    KEPT.has(key)
}

#[cfg(test)]
mod tests;
