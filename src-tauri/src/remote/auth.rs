//! Pairing tokens and device identities for the remote server.
//!
//! One secret and one identity. The secret is the pairing token the user reads
//! off the desktop (single use, five minutes). The identity is the phone's
//! Noise static public key, learned from the handshake and never from a frame:
//! there is no device token to steal, and `devices.json` holds only public
//! keys, so a copied file is not a credential.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rand::Rng;
use serde::{Deserialize, Serialize};

use super::secure::encode_key;
use crate::error::Result;

/// Pairing-token alphabet — `A-Z2-9` per the protocol doc. `0` and `1` are out
/// because they are the glyphs users misread as `O` and `I` when retyping a
/// code off a screen.
const PAIRING_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ23456789";
const PAIRING_LEN: usize = 8;

/// How long a minted pairing token stays redeemable.
pub const PAIRING_TTL: Duration = Duration::from_secs(5 * 60);

/// A freshly minted pairing token plus the wall-clock instant it lapses, which
/// is what Settings counts down to.
#[derive(Debug, Clone)]
pub struct MintedToken {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// The outstanding pairing tokens. Small by construction: a token is dropped
/// the moment it is redeemed, and every mint sweeps the lapsed ones.
pub struct PairingTokens {
    pending: Mutex<Vec<Pending>>,
}

struct Pending {
    token: String,
    /// Monotonic deadline, so a wall-clock jump can neither extend a token nor
    /// void one early.
    expires_at: Instant,
}

impl PairingTokens {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
        }
    }

    pub fn mint(&self) -> MintedToken {
        self.mint_with_ttl(PAIRING_TTL)
    }

    /// `ttl` is a parameter rather than the constant so tests can mint a token
    /// that is already past its deadline.
    pub fn mint_with_ttl(&self, ttl: Duration) -> MintedToken {
        let token = random_code(PAIRING_LEN);
        let now = Instant::now();
        let mut pending = self.pending.lock();
        pending.retain(|p| p.expires_at > now);
        pending.push(Pending {
            token: token.clone(),
            expires_at: now + ttl,
        });
        let expires_at = Utc::now() + chrono::Duration::from_std(ttl).unwrap_or_default();
        MintedToken { token, expires_at }
    }

    /// Redeem a token: true at most once per mint, and never past the TTL.
    pub fn consume(&self, token: &str) -> bool {
        let now = Instant::now();
        let mut pending = self.pending.lock();
        pending.retain(|p| p.expires_at > now);
        match pending.iter().position(|p| p.token == token) {
            Some(i) => {
                pending.remove(i);
                true
            }
            None => false,
        }
    }
}

impl Default for PairingTokens {
    fn default() -> Self {
        Self::new()
    }
}

fn random_code(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| PAIRING_ALPHABET[rng.random_range(0..PAIRING_ALPHABET.len())] as char)
        .collect()
}

/// One paired device as persisted in `devices.json`. camelCase on the wire and
/// on disk, matching the `RemoteDevice` DTO in the protocol doc.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRecord {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    /// The device's Noise static public key, base64url without padding. Public
    /// by definition: it authenticates the phone only because the phone holds
    /// the matching private key, which never leaves it.
    pub public_key: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

/// The paired-device registry, mirrored to `<dir>/devices.json`.
pub struct DeviceStore {
    path: PathBuf,
    devices: Mutex<Vec<DeviceRecord>>,
    /// Why the store is not usable, if it is not: it could not be opened, or
    /// the last write failed. Surfaced as `RemoteStatus.error` and cleared by
    /// the next write that succeeds. Pairing refuses while it is set, because
    /// a credential that cannot be persisted would silently stop working at
    /// the next launch. Lock order: `devices` first, then this.
    storage_error: Mutex<Option<String>>,
    /// Memory is ahead of disk: a write failed after the list changed. `flush`
    /// retries the write while this is set, so a transient failure (disk full,
    /// a permissions slip) heals on its own instead of resurrecting a revoked
    /// credential at the next launch.
    unsaved: AtomicBool,
}

impl DeviceStore {
    /// Open (or start) the store at `<dir>/devices.json`. Never fails: a file
    /// we can't parse, or a directory we can't create, is treated as an empty
    /// list rather than as fatal — the user can re-pair, whereas a refusal to
    /// launch (or an unmanaged state that panics the settings pane) would be
    /// unrecoverable from the UI. An unusable directory is remembered in
    /// `storage_error`.
    pub fn load(dir: &Path) -> Self {
        let mut storage_error = None;
        let mut devices = Vec::new();
        let path = dir.join("devices.json");
        if let Err(e) = std::fs::create_dir_all(dir) {
            storage_error = Some(format!(
                "Paired devices cannot be stored in {}: {e}",
                dir.display()
            ));
        } else {
            match std::fs::read(&path) {
                Ok(bytes) => devices = parse_devices(&bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    storage_error = Some(format!(
                        "Paired devices cannot be read from {}: {e}",
                        path.display()
                    ))
                }
            }
        }
        if let Some(error) = &storage_error {
            tracing::error!(%error, "remote: device store unavailable");
        }
        Self {
            path,
            devices: Mutex::new(devices),
            storage_error: Mutex::new(storage_error),
            unsaved: AtomicBool::new(false),
        }
    }

    pub fn storage_error(&self) -> Option<String> {
        self.storage_error.lock().clone()
    }

    /// Retry a write that failed earlier, if there is one outstanding. Called
    /// from `RemoteState::status`, which Settings polls, so the retry runs a
    /// few times a minute while the pane is open and on every host command.
    pub fn flush(&self) {
        if !self.unsaved.load(Ordering::Acquire) {
            return;
        }
        let devices = self.devices.lock();
        let _ = self.save(&devices);
    }

    pub fn list(&self) -> Vec<DeviceRecord> {
        self.devices.lock().clone()
    }

    /// Record a device under the static public key its handshake proved.
    ///
    /// One device, one record: pairing again with a key already on file updates
    /// that record's name and platform rather than adding a duplicate, since
    /// the key *is* the identity and a phone that re-pairs (a lapsed code, a
    /// reinstall that kept its key) is the same device.
    pub fn register(
        &self,
        name: &str,
        platform: &str,
        public_key: &[u8; 32],
    ) -> Result<DeviceRecord> {
        let public_key = encode_key(public_key);
        let now = Utc::now().to_rfc3339();
        self.mutate(move |devices| {
            if let Some(existing) = devices.iter_mut().find(|d| d.public_key == public_key) {
                existing.name = name.to_string();
                existing.platform = platform.to_string();
                return existing.clone();
            }
            let record = DeviceRecord {
                device_id: uuid::Uuid::new_v4().to_string(),
                name: name.to_string(),
                platform: platform.to_string(),
                public_key,
                created_at: now,
                last_seen_at: None,
            };
            devices.push(record.clone());
            record
        })
    }

    /// Resolve the static key the handshake proved to its record. `None` for a
    /// key that never paired or was revoked — the caller turns that into close
    /// code 4003.
    pub fn find_by_key(&self, public_key: &[u8; 32]) -> Option<DeviceRecord> {
        let public_key = encode_key(public_key);
        self.devices
            .lock()
            .iter()
            .find(|d| d.public_key == public_key)
            .cloned()
    }

    /// Whether a device is still registered. Used to re-check a credential
    /// after a connection has made itself revocable, closing the window where
    /// a revoke lands between `verify` and that registration.
    pub fn contains(&self, device_id: &str) -> bool {
        self.devices.lock().iter().any(|d| d.device_id == device_id)
    }

    /// Record that a device just authenticated. Best effort: a failed write
    /// costs a "last seen" timestamp, never a connection.
    pub fn touch(&self, device_id: &str) {
        let now = Utc::now().to_rfc3339();
        let written = self.mutate(|devices| {
            if let Some(d) = devices.iter_mut().find(|d| d.device_id == device_id) {
                d.last_seen_at = Some(now);
            }
        });
        if let Err(e) = written {
            tracing::warn!(error = %e, "remote: persisting last-seen failed");
        }
    }

    /// Drop a device's credential. Returns whether anything was removed.
    pub fn revoke(&self, device_id: &str) -> Result<bool> {
        self.mutate(|devices| {
            let before = devices.len();
            devices.retain(|d| d.device_id != device_id);
            devices.len() != before
        })
    }

    /// The one write path: apply `f` to the list and persist it *while still
    /// holding the lock*, so concurrent writers serialize.
    ///
    /// Every writer used to clone the list, release the lock and then write to
    /// the same `devices.json.tmp`, which let two writes rename each other's
    /// temp file and land out of order — a revoke overtaken by a `touch` would
    /// resurrect the revoked credential at the next launch.
    fn mutate<R>(&self, f: impl FnOnce(&mut Vec<DeviceRecord>) -> R) -> Result<R> {
        let mut devices = self.devices.lock();
        let out = f(&mut devices);
        self.save(&devices)?;
        Ok(out)
    }

    /// `persist` plus the bookkeeping that makes a failure visible and
    /// retryable: a failed write sets `storage_error` and `unsaved`, a
    /// successful one clears both. Caller holds the `devices` lock.
    fn save(&self, devices: &[DeviceRecord]) -> Result<()> {
        match self.persist(devices) {
            Ok(()) => {
                self.unsaved.store(false, Ordering::Release);
                *self.storage_error.lock() = None;
                Ok(())
            }
            Err(e) => {
                let error = format!(
                    "Paired devices could not be saved to {}: {e}. Revocations and new \
                     pairings will not survive a relaunch until this is fixed.",
                    self.path.display()
                );
                tracing::error!(%error, "remote: device store write failed");
                self.unsaved.store(true, Ordering::Release);
                *self.storage_error.lock() = Some(error);
                Err(e)
            }
        }
    }

    /// Write the registry atomically (tmp + rename) at 0600 — it is the list of
    /// devices that may drive this Mac, so it should not be world-readable (or
    /// world-writable) even inside the app data dir.
    fn persist(&self, devices: &[DeviceRecord]) -> Result<()> {
        let json = serde_json::to_vec_pretty(devices)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// Records are read one at a time so a single unusable one costs only itself.
///
/// The case that matters is the token era: a record with a `tokenHash` and no
/// `publicKey` cannot authenticate anything under v2, and the doc has it
/// dropped at load. The device re-pairs, which is the only way it could work
/// again anyway.
fn parse_devices(bytes: &[u8]) -> Vec<DeviceRecord> {
    let raw: Vec<serde_json::Value> = match serde_json::from_slice(bytes) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!(error = %e, "remote: unreadable devices.json; starting empty");
            return Vec::new();
        }
    };
    let total = raw.len();
    let devices: Vec<DeviceRecord> = raw
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();
    if devices.len() != total {
        tracing::warn!(
            dropped = total - devices.len(),
            "remote: dropped device records with no public key; those devices must pair again"
        );
    }
    devices
}
