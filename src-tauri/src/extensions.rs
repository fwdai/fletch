//! Extension backend runtime.
//!
//! Extensions live under the repo-root `extensions/` directory. Their backends
//! (`backend/mod.rs`) and migrations (`migrations/*.sql`) are discovered at
//! build time and stitched in by `build.rs` (see the `include!` at the bottom),
//! so an extension needs no Cargo crate of its own and the open-source build
//! compiles fine with no extensions present.
//!
//! Backend commands are dispatched through a single Tauri command, `ext_invoke`
//! — an app-level command, so it needs no per-extension capability/ACL entry,
//! unlike a Tauri plugin would. An extension registers handlers in its
//! `register(&mut Registrar)` fn:
//!
//! ```ignore
//! use crate::extensions::prelude::*;
//! pub fn register(api: &mut Registrar) {
//!     api.command("count_notes", |_args, ctx| {
//!         let n: i64 = ctx.db.query_row("SELECT COUNT(*) FROM demo_notes", (), |r| r.get(0))
//!             .map_err(|e| e.to_string())?;
//!         Ok(json!({ "count": n }))
//!     });
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

/// What an extension backend imports: `use crate::extensions::prelude::*;`.
/// A convenience re-export surface; which items a given build actually uses
/// depends on which extensions are present, so don't warn on unused ones.
#[allow(unused_imports)]
pub mod prelude {
    pub use super::{CmdResult, ExtensionContext, Registrar};
    pub use rusqlite::Connection;
    pub use serde_json::{json, Value};
}

/// Return type of an extension command handler: any JSON value, or an error
/// string surfaced to the frontend as a rejected promise.
pub type CmdResult = Result<Value, String>;

/// Everything a handler is given at call time. Holds the locked DB connection
/// (extensions read/write their own tables through it) and the app handle.
// Fields are handler-facing API; a build with no (or DB-only) extensions reads
// neither, so allow them to go unread.
#[allow(dead_code)]
pub struct ExtensionContext<'a> {
    pub db: &'a Connection,
    /// Available to extension handlers (events, dialogs, paths…).
    pub app: &'a tauri::AppHandle,
}

type Handler = Box<dyn Fn(Value, &ExtensionContext) -> CmdResult + Send + Sync>;

/// Registry of every extension command, keyed by `(extension_id, command)`.
/// Built once at startup from the generated `register_all`, then shared
/// (immutably) as Tauri state and consulted by `ext_invoke`.
#[derive(Default)]
pub struct ExtensionApi {
    handlers: HashMap<(String, String), Handler>,
}

impl ExtensionApi {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scope registration to one extension so its `register` fn doesn't repeat
    /// its own id on every command. Unused in a build with no extensions.
    #[allow(dead_code)]
    pub fn scope(&mut self, ext: &str) -> Registrar<'_> {
        Registrar { api: self, ext: ext.to_string() }
    }

    fn dispatch(&self, ext: &str, cmd: &str, args: Value, ctx: &ExtensionContext) -> CmdResult {
        match self.handlers.get(&(ext.to_string(), cmd.to_string())) {
            Some(handler) => handler(args, ctx),
            None => Err(format!("unknown extension command: {ext}:{cmd}")),
        }
    }
}

/// Handed to an extension's `register` fn. `command` is the entire surface an
/// extension uses to add backend functionality. Unused in a build with no
/// extensions, hence the allow.
#[allow(dead_code)]
pub struct Registrar<'a> {
    api: &'a mut ExtensionApi,
    ext: String,
}

#[allow(dead_code)]
impl Registrar<'_> {
    pub fn command<F>(&mut self, name: &str, handler: F)
    where
        F: Fn(Value, &ExtensionContext) -> CmdResult + Send + Sync + 'static,
    {
        self.api
            .handlers
            .insert((self.ext.clone(), name.to_string()), Box::new(handler));
    }
}

/// One embedded extension migration (filled in by the generated glue).
pub struct ExtMigration {
    pub extension: &'static str,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Apply any not-yet-applied extension migrations. Tracked in a dedicated
/// `_ext_migrations` table — independent of the core's `user_version` — so the
/// set of installed extensions can differ per machine and change across builds
/// without confusing the core schema version. Idempotent.
pub fn apply_extension_migrations(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _ext_migrations (
            extension  TEXT NOT NULL,
            name       TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (extension, name)
        );",
    )?;

    for m in extension_migrations() {
        let applied: Option<bool> = conn
            .query_row(
                "SELECT 1 FROM _ext_migrations WHERE extension = ?1 AND name = ?2",
                (m.extension, m.name),
                |_| Ok(true),
            )
            .optional()?;
        if applied.is_some() {
            continue;
        }
        // Apply the migration and record it in one transaction so it can never
        // be left applied-but-unrecorded (a crash or failed INSERT mid-way rolls
        // back the whole thing). Otherwise a re-run on next launch would break
        // any non-idempotent migration — and bring down boot, since lib.rs
        // `.expect()`s this. Mirrors rusqlite_migration's per-step atomicity.
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(m.sql)?;
        tx.execute(
            "INSERT INTO _ext_migrations (extension, name) VALUES (?1, ?2)",
            (m.extension, m.name),
        )?;
        tx.commit()?;
        tracing::info!(extension = m.extension, migration = m.name, "applied extension migration");
    }
    Ok(())
}

/// Build the registry from the generated glue. Called once at startup.
pub fn build_api() -> ExtensionApi {
    let mut api = ExtensionApi::new();
    register_all(&mut api);
    api
}

/// Single IPC entry point for all extension backend calls. App-level (not a
/// plugin command), so no per-extension ACL is needed. The frontend reaches it
/// via `callExtension(extension, command, args)`.
// `async` so it runs on Tauri's worker pool (like the core `db_*` commands)
// rather than the main thread, where a slow handler would freeze the UI. Safe
// despite the `!Send` lock guard: there are no await points while it is held.
#[tauri::command]
pub async fn ext_invoke(
    extension: String,
    command: String,
    args: Value,
    app: tauri::AppHandle,
    db: tauri::State<'_, Arc<Mutex<Connection>>>,
    api: tauri::State<'_, Arc<ExtensionApi>>,
) -> CmdResult {
    let conn = db.lock();
    let ctx = ExtensionContext { db: &conn, app: &app };
    api.dispatch(&extension, &command, args, &ctx)
}

// Generated by build.rs: `mod ext_<name>;` for each backend, plus
// `register_all` and `extension_migrations`. Empty (valid no-ops) when no
// extensions are present.
include!(concat!(env!("OUT_DIR"), "/extensions_glue.rs"));
