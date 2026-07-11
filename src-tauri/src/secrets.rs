//! App-held secrets (the GitHub OAuth token, the claude container token).
//!
//! Release macOS builds store secrets in the login Keychain through the
//! in-process Security.framework API: the item is created and read by the
//! same signed binary, so the app's own access is silent (no prompt, no
//! password), while any *other* process — including a sandboxed agent, whose
//! seatbelt profile leaves file reads open — hits a visible macOS consent
//! dialog instead of reading silently. Dev builds (and non-macOS targets)
//! fall back to the plaintext `settings` table: an ad-hoc-signed dev binary
//! changes code identity on every rebuild, which would otherwise re-prompt
//! each run. Values that reached the settings table under the pre-keychain
//! scheme are migrated (and the plaintext scrubbed) on first read.
//!
//! `get` keeps a missing secret (`Ok(None)`) distinct from an unavailable
//! store (`Err` — e.g. a locked keychain): a saved login that can't be read
//! right now must surface as a retryable failure, never as signed-out.
//!
//! Secrets read or written here must never appear in logs, telemetry, or
//! error strings — the `github::client` invariant, enforced at the store too.

use rusqlite::Connection;

use crate::error::Result;

pub use store::{delete, get, set};

/// Keychain-backed store (release macOS). The setting key doubles as the
/// keychain account name; the service is the app bundle id.
#[cfg(all(target_os = "macos", not(debug_assertions)))]
mod store {
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    use super::*;
    use crate::database;
    use crate::error::Error;

    const SERVICE: &str = crate::BUNDLE_ID;

    /// `errSecItemNotFound` — the one read/delete failure that means "no such
    /// secret". Every other code (locked keychain, interaction not allowed)
    /// means the secret may exist but the keychain is unavailable right now,
    /// and must not be treated as absence.
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
        let err = match get_generic_password(SERVICE, key) {
            Ok(bytes) => {
                return Ok(String::from_utf8(bytes)
                    .ok()
                    .filter(|v| !v.trim().is_empty()))
            }
            Err(e) => e,
        };
        // The legacy plaintext `settings` row a pre-keychain build left
        // behind — consulted both for migration and as the working copy
        // while the keychain is unavailable.
        let legacy = database::get_setting(conn, key).filter(|v| !v.trim().is_empty());
        if err.code() != ERR_SEC_ITEM_NOT_FOUND {
            // Unavailable ≠ absent (locked keychain, interaction not
            // allowed): a migrated secret may exist but can't be read right
            // now. A still-unmigrated legacy value keeps the session working
            // (no migration attempt — the keychain write would fail too);
            // otherwise surface the failure so callers retry instead of
            // treating the account as signed out.
            return match legacy {
                Some(v) => Ok(Some(v)),
                None => Err(Error::Other(format!("keychain read ({key}): {err}"))),
            };
        }
        // Definitively absent from the keychain: migrate the legacy row,
        // then scrub it. On a keychain write failure keep the settings copy —
        // the token still works, and the move retries on the next read. A
        // failed scrub is non-fatal *here* (the keychain copy is in place and
        // the value must still be returned); `delete` is where a surviving
        // row is load-bearing.
        let Some(legacy) = legacy else { return Ok(None) };
        match set(conn, key, &legacy) {
            Ok(()) => {
                if let Err(e) = scrub_setting(conn, key) {
                    tracing::warn!(key, error = %e, "failed to blank migrated plaintext row");
                }
            }
            Err(e) => tracing::warn!(key, error = %e, "keychain migration failed"),
        }
        Ok(Some(legacy))
    }

    pub fn set(_conn: &Connection, key: &str, value: &str) -> Result<()> {
        set_generic_password(SERVICE, key, value.as_bytes())
            .map_err(|e| Error::Other(format!("keychain write ({key}): {e}")))
    }

    pub fn delete(conn: &Connection, key: &str) -> Result<()> {
        match delete_generic_password(SERVICE, key) {
            // Deleting an absent secret is a no-op, not an error.
            Ok(()) => {}
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => {}
            // Any other failure must fail the disconnect: swallowing it would
            // clear only the in-process mirror, and the surviving keychain
            // item would re-seed the token on next launch — a "disconnected"
            // account that signs itself back in.
            Err(e) => return Err(Error::Other(format!("keychain delete ({key}): {e}"))),
        }
        // Blank any plaintext row a failed migration could have left behind —
        // and fail the disconnect if that blank fails: a surviving row would
        // be re-migrated into the keychain on the next read, resurrecting the
        // "deleted" secret.
        if database::get_setting(conn, key).is_some_and(|v| !v.is_empty()) {
            scrub_setting(conn, key)?;
        }
        Ok(())
    }

    /// Blank the legacy `settings` row, then scrub the old plaintext from the
    /// database *file*: a plain UPDATE leaves the value recoverable on freed
    /// pages and in the WAL, so zero freed content (`secure_delete`), rewrite
    /// the main file (VACUUM), and drain the WAL (checkpoint TRUNCATE).
    /// The row blank is the load-bearing step — a surviving value is a live
    /// secret `get` would re-migrate — so its failure propagates; the file
    /// scrub guards against forensic *remnants* and stays best-effort.
    fn scrub_setting(conn: &Connection, key: &str) -> Result<()> {
        // Armed before the overwrite so the freed page content is zeroed;
        // only weakens the file scrub if it fails, so best-effort.
        if let Err(e) = conn.pragma_update(None, "secure_delete", "ON") {
            tracing::warn!(key, error = %e, "failed to enable secure_delete");
        }
        database::set_setting(conn, key, "")?;
        let file_scrub = || -> Result<()> {
            conn.execute_batch("VACUUM")?;
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
            Ok(())
        };
        if let Err(e) = file_scrub() {
            tracing::warn!(key, error = %e, "failed to scrub plaintext remnants from db file");
        }
        Ok(())
    }
}

/// `settings`-table store (dev builds and non-macOS): the pre-keychain
/// plaintext posture, kept where keychain ACLs can't work — an ad-hoc-signed
/// dev binary changes identity every rebuild and would prompt each run.
#[cfg(not(all(target_os = "macos", not(debug_assertions))))]
mod store {
    use super::*;
    use crate::database;

    pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
        Ok(database::get_setting(conn, key).filter(|v| !v.trim().is_empty()))
    }

    pub fn set(conn: &Connection, key: &str, value: &str) -> Result<()> {
        database::set_setting(conn, key, value)
    }

    pub fn delete(conn: &Connection, key: &str) -> Result<()> {
        database::set_setting(conn, key, "")
    }
}

// Tests build with `debug_assertions`, so they exercise the settings-table
// store — the keychain store is thin glue over Security.framework that can't
// run headless (no login keychain in CI).
#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> (
        tempfile::TempDir,
        std::sync::Arc<parking_lot::Mutex<Connection>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::database::init(dir.path()).unwrap();
        (dir, db)
    }

    #[test]
    fn roundtrip_set_get_delete() {
        let (_dir, db) = test_db();
        let conn = db.lock();
        assert_eq!(get(&conn, "github_token").unwrap(), None);
        set(&conn, "github_token", "tok123").unwrap();
        assert_eq!(get(&conn, "github_token").unwrap().as_deref(), Some("tok123"));
        delete(&conn, "github_token").unwrap();
        assert_eq!(get(&conn, "github_token").unwrap(), None);
    }

    /// A cleared secret persists as a blank row (dev store) — blank must read
    /// back as "no secret" (`Ok(None)`, not an error), matching
    /// `github::set_token`'s blank handling.
    #[test]
    fn blank_value_reads_as_none() {
        let (_dir, db) = test_db();
        let conn = db.lock();
        set(&conn, "claude_container_token", "  ").unwrap();
        assert_eq!(get(&conn, "claude_container_token").unwrap(), None);
    }
}
