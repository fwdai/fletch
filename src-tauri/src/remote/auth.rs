//! Pairing tokens and device credentials for the remote server.
//!
//! Two secrets with two lifetimes: a short pairing token the user reads off the
//! desktop (single use, five minutes) and a long-lived device token the phone
//! keeps. Only the device token's sha256 is ever written to disk, so a copied
//! `devices.json` can't be replayed as a credential.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;

/// Pairing-token alphabet — `A-Z2-9` per the protocol doc. `0` and `1` are out
/// because they are the glyphs users misread as `O` and `I` when retyping a
/// code off a screen.
const PAIRING_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ23456789";
const PAIRING_LEN: usize = 8;

/// How long a minted pairing token stays redeemable.
pub const PAIRING_TTL: Duration = Duration::from_secs(5 * 60);

/// Device-token entropy: 32 bytes, which is 43 base64url characters.
const DEVICE_TOKEN_BYTES: usize = 32;

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
    /// Hex sha256 of the device token. The plaintext exists only in the `pair`
    /// response and on the phone.
    pub token_hash: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

/// The paired-device registry, mirrored to `<dir>/devices.json`.
pub struct DeviceStore {
    path: PathBuf,
    devices: Mutex<Vec<DeviceRecord>>,
    /// Why the store could not be opened, if it could not. Surfaced as
    /// `RemoteStatus.error`; pairing refuses while it is set, because a
    /// credential that cannot be persisted would silently stop working at the
    /// next launch.
    storage_error: Option<String>,
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
                Ok(bytes) => devices = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "remote: unreadable devices.json; starting empty");
                    Vec::new()
                }),
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
            storage_error,
        }
    }

    pub fn storage_error(&self) -> Option<&str> {
        self.storage_error.as_deref()
    }

    pub fn list(&self) -> Vec<DeviceRecord> {
        self.devices.lock().clone()
    }

    /// Register a device and hand back its one-and-only plaintext token.
    pub fn register(&self, name: &str, platform: &str) -> Result<(DeviceRecord, String)> {
        let mut bytes = [0u8; DEVICE_TOKEN_BYTES];
        rand::rng().fill_bytes(&mut bytes);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let record = DeviceRecord {
            device_id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            platform: platform.to_string(),
            token_hash: hash_token(&token),
            created_at: Utc::now().to_rfc3339(),
            last_seen_at: None,
        };
        self.mutate(|devices| devices.push(record.clone()))?;
        Ok((record, token))
    }

    /// Resolve a presented device token to its record. `None` for an unknown or
    /// revoked token — the caller turns that into close code 4003.
    pub fn verify(&self, token: &str) -> Option<DeviceRecord> {
        let hash = hash_token(token);
        self.devices
            .lock()
            .iter()
            .find(|d| d.token_hash == hash)
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
        self.persist(&devices)?;
        Ok(out)
    }

    /// Write the registry atomically (tmp + rename) at 0600 — it holds token
    /// hashes, so it should not be world-readable even inside the app data dir.
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

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
