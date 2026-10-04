//! The per-project settings a client may read and write: the rows in
//! `project_settings` that the host's own loops read (the turn-end verifier,
//! the Run panel, the roadmap queue, the issue funnel), reached through one
//! allowlisted op pair instead of the generic `db_*` bridge. The desktop's
//! Tauri commands of the same names call these too, so a project page edits the
//! host it is driving whether that is this Mac or a paired one.
//!
//! See docs/remote-protocol.md, "Settings".

use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::host::EngineCtx;

/// The exact keys a client may read and write. Each is read host-side; the
/// readers are named in the protocol doc's table.
const PROJECT_SETTING_KEYS: &[&str] = &[
    "verify.on_turn_end",
    crate::run_env::RUN_ENV_SETTING,
    "workflow.default",
    "composer.mode",
    "roadmap.autoqueue",
    "roadmap.max_concurrent",
    "roadmap.settle_review",
    "roadmap.midrun_awareness",
    "roadmap.declined_issues",
    "linear.team_id",
    "linear.team_name",
];

/// The key family a client may write in full: `run.<row>` (the project's run
/// commands) and `run.agent.<agentId>.<row>` (one agent's overrides over them).
const RUN_PREFIX: &str = "run.";

/// The event a write emits, one per key, forwarded to every paired client.
pub const PROJECT_SETTINGS_CHANGED: &str = "project_settings:changed";

/// Whether a client may read or write `key`. Everything else in the table —
/// the roadmap's code allocator, autopilot's switch, anything a later release
/// adds — stays the host's own until it is put here on purpose.
pub fn is_client_project_key(key: &str) -> bool {
    PROJECT_SETTING_KEYS.contains(&key)
        || (key.starts_with(RUN_PREFIX) && key.len() > RUN_PREFIX.len())
}

#[derive(Serialize)]
struct Changed<'a> {
    project_id: &'a str,
    key: &'a str,
    value: Option<&'a str>,
}

/// The project's client-writable settings. An absent key is unset and reads as
/// its default on both sides.
pub fn get_project_settings_impl(
    ctx: &EngineCtx,
    project_id: &str,
) -> Result<BTreeMap<String, String>> {
    let conn = ctx.db.lock();
    let mut stmt = conn.prepare("SELECT key, value FROM project_settings WHERE project_id = ?1")?;
    let rows = stmt.query_map([project_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (key, value) = row?;
        if is_client_project_key(&key) {
            out.insert(key, value);
        }
    }
    Ok(out)
}

/// Write one client-writable key, or delete its row for `None` (an absent row
/// is how every key spells its default). Refuses a key off the allowlist
/// before touching the table, then tells every other view.
pub fn set_project_setting_impl(
    ctx: &EngineCtx,
    project_id: &str,
    key: &str,
    value: Option<&str>,
) -> Result<()> {
    if !is_client_project_key(key) {
        return Err(Error::Other(format!(
            "`{key}` is not a project setting a client may write"
        )));
    }
    {
        let conn = ctx.db.lock();
        match value {
            Some(v) => conn.execute(
                "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
                rusqlite::params![project_id, key, v],
            )?,
            None => conn.execute(
                "DELETE FROM project_settings WHERE project_id = ?1 AND key = ?2",
                rusqlite::params![project_id, key],
            )?,
        };
    }
    crate::host::emit(
        ctx.sink.as_ref(),
        PROJECT_SETTINGS_CHANGED,
        &Changed {
            project_id,
            key,
            value,
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ctx::test_ctx;
    use serde_json::json;

    fn with_project(ctx: &EngineCtx) {
        ctx.db
            .lock()
            .execute(
                "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'p', 0)",
                [],
            )
            .unwrap();
    }

    /// Every key the audit found a host loop reading and a client writing.
    #[test]
    fn every_host_read_client_key_is_allowed() {
        for key in [
            "verify.on_turn_end",
            "run.test",
            "run.install",
            "run.lint",
            "run.dev",
            "run.agent.fuji.test",
            "run_env",
            "workflow.default",
            "composer.mode",
            "roadmap.autoqueue",
            "roadmap.max_concurrent",
            "roadmap.settle_review",
            "roadmap.midrun_awareness",
            "roadmap.declined_issues",
            "linear.team_id",
            "linear.team_name",
        ] {
            assert!(is_client_project_key(key), "{key} must be client-writable");
        }
    }

    /// The host's own rows, and autopilot's switch (the desktop loop's, with an
    /// op of its own), are neither answered nor writable.
    #[test]
    fn host_internal_and_unlisted_keys_are_refused() {
        for key in [
            "roadmap.code_prefix",
            "roadmap.code_seq",
            "autopilot.enabled",
            "run.",
            "runx",
            "",
            "verify.on_turn_end.extra",
        ] {
            assert!(!is_client_project_key(key), "{key} must be refused");
        }
    }

    #[test]
    fn a_write_round_trips_emits_and_null_deletes() {
        let (ctx, sink, _dir) = test_ctx();
        with_project(&ctx);

        set_project_setting_impl(&ctx, "p1", "verify.on_turn_end", Some("1")).unwrap();
        assert_eq!(
            get_project_settings_impl(&ctx, "p1").unwrap(),
            BTreeMap::from([("verify.on_turn_end".to_string(), "1".to_string())])
        );
        set_project_setting_impl(&ctx, "p1", "verify.on_turn_end", None).unwrap();
        assert!(get_project_settings_impl(&ctx, "p1").unwrap().is_empty());

        assert_eq!(
            sink.events(),
            vec![
                (
                    PROJECT_SETTINGS_CHANGED.to_string(),
                    json!({ "project_id": "p1", "key": "verify.on_turn_end", "value": "1" })
                ),
                (
                    PROJECT_SETTINGS_CHANGED.to_string(),
                    json!({ "project_id": "p1", "key": "verify.on_turn_end", "value": null })
                ),
            ]
        );
    }

    #[test]
    fn a_refused_key_writes_nothing_and_says_which() {
        let (ctx, sink, _dir) = test_ctx();
        with_project(&ctx);
        let err = set_project_setting_impl(&ctx, "p1", "roadmap.code_seq", Some("9")).unwrap_err();
        assert!(err.to_string().contains("roadmap.code_seq"));
        let n: i64 = ctx
            .db
            .lock()
            .query_row("SELECT COUNT(*) FROM project_settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        assert!(sink.events().is_empty());
    }

    /// The read is filtered too: a host-internal row in the same project is not
    /// answered.
    #[test]
    fn the_read_answers_only_allowlisted_rows() {
        let (ctx, _sink, _dir) = test_ctx();
        with_project(&ctx);
        ctx.db
            .lock()
            .execute(
                "INSERT INTO project_settings (project_id, key, value) VALUES \
                 ('p1', 'roadmap.code_seq', '7'), ('p1', 'autopilot.enabled', '0'), \
                 ('p1', 'run.test', 'bun test')",
                [],
            )
            .unwrap();
        assert_eq!(
            get_project_settings_impl(&ctx, "p1").unwrap(),
            BTreeMap::from([("run.test".to_string(), "bun test".to_string())])
        );
    }
}
