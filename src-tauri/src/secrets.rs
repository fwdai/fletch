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

    pub fn get(conn: &Connection, key: &str) -> Option<String> {
        if let Ok(bytes) = get_generic_password(SERVICE, key) {
            return String::from_utf8(bytes)
                .ok()
                .filter(|v| !v.trim().is_empty());
        }
        // Not in the keychain: migrate the legacy plaintext `settings` row a
        // pre-keychain build left behind, then scrub it. On a keychain write
        // failure keep the settings copy — the token still works, and the
        // move retries on the next read.
        let legacy = database::get_setting(conn, key).filter(|v| !v.trim().is_empty())?;
        match set(conn, key, &legacy) {
            Ok(()) => scrub_setting(conn, key),
            Err(e) => tracing::warn!(key, error = %e, "keychain migration failed"),
        }
        Some(legacy)
    }

    pub fn set(_conn: &Connection, key: &str, value: &str) -> Result<()> {
        set_generic_password(SERVICE, key, value.as_bytes())
            .map_err(|e| Error::Other(format!("keychain write ({key}): {e}")))
    }

    pub fn delete(conn: &Connection, key: &str) -> Result<()> {
        // Deleting an absent secret is a no-op, not an error.
        let _ = delete_generic_password(SERVICE, key);
        // Scrub any plaintext row a failed migration could have left behind.
        if database::get_setting(conn, key).is_some_and(|v| !v.is_empty()) {
            scrub_setting(conn, key);
        }
        Ok(())
    }

    /// Blank the legacy `settings` row and scrub the old plaintext from the
    /// database *file*: a plain UPDATE leaves the value recoverable on freed
    /// pages and in the WAL, so zero freed content (`secure_delete`), rewrite
    /// the main file (VACUUM), and drain the WAL (checkpoint TRUNCATE).
    /// Best-effort — the secret is already safe in the keychain, so a scrub
    /// failure warns rather than blocking the caller.
    fn scrub_setting(conn: &Connection, key: &str) {
        let scrub = || -> Result<()> {
            conn.pragma_update(None, "secure_delete", "ON")?;
            database::set_setting(conn, key, "")?;
            conn.execute_batch("VACUUM")?;
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
            Ok(())
        };
        if let Err(e) = scrub() {
            tracing::warn!(key, error = %e, "failed to scrub legacy plaintext setting");
        }
    }
}

/// `settings`-table store (dev builds and non-macOS): the pre-keychain
/// plaintext posture, kept where keychain ACLs can't work — an ad-hoc-signed
/// dev binary changes identity every rebuild and would prompt each run.
#[cfg(not(all(target_os = "macos", not(debug_assertions))))]
mod store {
    use super::*;
    use crate::database;

    pub fn get(conn: &Connection, key: &str) -> Option<String> {
        database::get_setting(conn, key).filter(|v| !v.trim().is_empty())
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
        assert_eq!(get(&conn, "github_token"), None);
        set(&conn, "github_token", "tok123").unwrap();
        assert_eq!(get(&conn, "github_token").as_deref(), Some("tok123"));
        delete(&conn, "github_token").unwrap();
        assert_eq!(get(&conn, "github_token"), None);
    }

    /// A cleared secret persists as a blank row (dev store) — blank must read
    /// back as "no secret", matching `github::set_token`'s blank handling.
    #[test]
    fn blank_value_reads_as_none() {
        let (_dir, db) = test_db();
        let conn = db.lock();
        set(&conn, "claude_container_token", "  ").unwrap();
        assert_eq!(get(&conn, "claude_container_token"), None);
    }
}
