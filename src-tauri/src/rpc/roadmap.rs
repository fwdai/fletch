//! The roadmap RPC ops: how the project-manager chat reads the board and puts
//! tickets on it.
//!
//! Same shape as [`crate::workflow::comms::WorkflowCommsDispatcher`]: this
//! dispatcher wraps the standard [`GitDispatcher`], owns the `roadmap_*` ops,
//! and delegates everything else — so the PM keeps the same read-only git
//! surface every other agent has (its `AgentCaps::advisory()` still refuses the
//! publish ops, one mechanism checked once).
//!
//! Two ops, both scoped to the project this chat belongs to. The project id is
//! stamped at construction from the workspace record, never taken from `args`:
//! a chat can only ever read and write its own project's board.
//!
//! - `roadmap_list` — the whole board, compact, including `done` items (the PM
//!   needs to know what already shipped before it proposes more).
//! - `roadmap_propose` — creates rows with `status = "proposed"`, `source =
//!   "pm"`. A proposed row is a *ghost* on the board: it renders where it would
//!   land, counts for nothing, and only becomes real when the user accepts it
//!   (`proposed → open`) or vanishes when they discard it. That is the whole
//!   safety property of this tool — the agent can suggest, never commit.
//!
//! Validation rejects the whole batch rather than creating a partial one: the
//! PM gets one precise error it can fix and retry, and the user never sees half
//! a proposal. The inserts run in a transaction for the same reason.

use std::collections::HashSet;

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tauri::AppHandle;

use crate::roadmap::store;
use crate::roadmap::types::{Horizon, ItemSize, ItemSource, ItemStatus, NewItem, RoadmapItem};
use crate::roadmap::Db;
use crate::rpc::git::GitDispatcher;
use crate::rpc::{Response, RpcDispatcher, RpcEvent, RpcFuture};

/// The ops this dispatcher owns. Pinned by a test against the instruction block
/// so the two can't drift — an agent told about an op that doesn't exist (or
/// given one it was never told about) is a silently broken tool.
pub const OPS: [&str; 2] = ["roadmap_list", "roadmap_propose"];

/// Most items one `roadmap_propose` call may carry. A proposal is a thing a
/// human reads and accepts; past a score of rows that stops being true, and the
/// PM should be slicing rather than dumping a backlog.
const MAX_BATCH: usize = 20;

/// Is `op` one this dispatcher owns? The whole `roadmap_` namespace, not just
/// the two known names, so a typo'd op gets a precise error naming the real
/// ones instead of the git dispatcher's generic "unknown op".
fn is_roadmap_op(op: &str) -> bool {
    op.starts_with("roadmap_")
}

// ───────────────────────────── arg shapes ───────────────────────────────

/// `roadmap_list` args. Everything optional: no args at all is the common call.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// Keep only these statuses. Absent means every row.
    #[serde(default)]
    status: Option<Vec<String>>,
}

/// `roadmap_propose` args.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeArgs {
    #[serde(default)]
    items: Vec<ProposedItem>,
}

/// One proposed ticket, as the agent writes it. Deliberately *not* [`NewItem`]:
/// the agent may not choose `status` or `source` (they are what makes this a
/// proposal), and unknown fields are rejected rather than silently dropped —
/// a misspelled `horizen` must fail loudly, not put the item in the backlog.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposedItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    why: String,
    #[serde(default)]
    horizon: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    area: Option<String>,
    #[serde(default)]
    epic: Option<String>,
    #[serde(default)]
    accept: Vec<String>,
    #[serde(default)]
    deps: Vec<String>,
}

/// Decode `args` into an op's shape. A missing `args` arrives as JSON null,
/// which is the same as `{}` for every op here.
fn parse_args<T: Default + serde::de::DeserializeOwned>(args: &Value) -> Result<T, String> {
    if args.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(args.clone()).map_err(|e| e.to_string())
}

/// Trim, and treat an empty string as absent — an agent that fills a field with
/// `""` means "no value", and storing that would show as an empty tag.
fn clean(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Trim a list and drop the blanks.
fn clean_list(v: &[String]) -> Vec<String> {
    v.iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The valid spellings of an enum, for an error message the agent can act on.
fn one_of(values: &[&str]) -> String {
    values.join(" | ")
}

// ───────────────────────────── roadmap_list ─────────────────────────────

/// One row as the agent sees it: the fields it reasons about, with the empties
/// omitted. Not [`RoadmapItem`]'s full serialization — ids, timestamps and run
/// back-links are the app's business, and the agent addresses items by `code`.
fn compact(item: &RoadmapItem) -> Value {
    let mut o = Map::new();
    o.insert("code".into(), json!(item.code));
    o.insert("title".into(), json!(item.title));
    o.insert("horizon".into(), json!(item.horizon.as_str()));
    o.insert("status".into(), json!(item.status.as_str()));
    if !item.why.is_empty() {
        o.insert("why".into(), json!(item.why));
    }
    if let Some(size) = item.size {
        o.insert("size".into(), json!(size.as_str()));
    }
    if let Some(area) = &item.area {
        o.insert("area".into(), json!(area));
    }
    if let Some(epic) = &item.epic {
        o.insert("epic".into(), json!(epic));
    }
    if !item.accept.is_empty() {
        o.insert("accept".into(), json!(item.accept));
    }
    if !item.deps.is_empty() {
        o.insert("deps".into(), json!(item.deps));
    }
    Value::Object(o)
}

/// `roadmap_list`: the project's board as a JSON array on stdout.
///
/// Pure over the connection so it is testable without an app handle; the
/// dispatcher holds the lock around it.
fn list_op(conn: &Connection, project_id: &str, id: &str, args: &Value) -> Response {
    let args: ListArgs = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return Response::err(id, format!("roadmap_list: {e}")),
    };
    let filter = match &args.status {
        None => None,
        Some(raw) => {
            let mut set = HashSet::new();
            for s in raw {
                match ItemStatus::from_db(s.trim()) {
                    Some(st) => {
                        set.insert(st.as_str());
                    }
                    None => {
                        return Response::err(
                            id,
                            format!(
                                "roadmap_list: unknown status {s:?} — expected {}",
                                one_of(&[
                                    "proposed",
                                    "open",
                                    "queued",
                                    "active",
                                    "in_review",
                                    "done"
                                ])
                            ),
                        )
                    }
                }
            }
            Some(set)
        }
    };

    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return Response::err(id, format!("roadmap_list: {e}")),
    };
    let keep = |i: &RoadmapItem| match &filter {
        None => true,
        Some(f) => f.contains(i.status.as_str()),
    };
    let rows: Vec<Value> = items.iter().filter(|i| keep(i)).map(compact).collect();
    match serde_json::to_string(&rows) {
        Ok(stdout) => Response::ok(id, 0, stdout, String::new()),
        Err(e) => Response::err(id, format!("roadmap_list: {e}")),
    }
}

// ─────────────────────────── roadmap_propose ────────────────────────────

/// Turn the agent's items into validated [`NewItem`]s, or explain what's wrong
/// with the batch. `known` is every code already on this project's board —
/// `deps` may only reference those (codes are allocated on insert, so an item
/// cannot depend on one from the same batch).
fn validate(items: &[ProposedItem], known: &HashSet<&str>) -> Result<Vec<NewItem>, String> {
    if items.is_empty() {
        return Err("`items` must be a non-empty array of tickets".into());
    }
    if items.len() > MAX_BATCH {
        return Err(format!(
            "{} items is too many for one proposal (max {MAX_BATCH}) — propose the next slice \
             once these are accepted",
            items.len()
        ));
    }
    let mut out = Vec::with_capacity(items.len());
    for (n, it) in items.iter().enumerate() {
        // 1-based: "item 1" is the first thing the agent wrote.
        let at = n + 1;
        let title = it.title.trim();
        if title.is_empty() {
            return Err(format!("item {at}: `title` is required"));
        }
        let horizon = match it.horizon.as_deref().map(str::trim) {
            None | Some("") => {
                return Err(format!(
                    "item {at} ({title:?}): `horizon` is required — {}",
                    one_of(&["now", "next", "later"])
                ))
            }
            Some(h) => Horizon::from_db(h).ok_or_else(|| {
                format!(
                    "item {at} ({title:?}): unknown horizon {h:?} — expected {}",
                    one_of(&["now", "next", "later"])
                )
            })?,
        };
        let size = match it.size.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(s) => Some(ItemSize::from_db(s).ok_or_else(|| {
                format!(
                    "item {at} ({title:?}): unknown size {s:?} — expected {}",
                    one_of(&["XS", "S", "M", "L"])
                )
            })?),
        };
        let deps = clean_list(&it.deps);
        for d in &deps {
            if !known.contains(d.as_str()) {
                return Err(format!(
                    "item {at} ({title:?}): `deps` names {d:?}, which is not an item on this \
                     board — depend only on codes `roadmap_list` returns (a ticket from this \
                     same batch has no code yet)"
                ));
            }
        }
        out.push(NewItem {
            title: title.to_string(),
            why: it.why.trim().to_string(),
            horizon: Some(horizon),
            // What makes this a proposal rather than a roadmap item: it lands
            // as a ghost the user has to accept.
            status: Some(ItemStatus::Proposed),
            size,
            area: clean(it.area.as_deref()),
            source: Some(ItemSource::Pm),
            epic: clean(it.epic.as_deref()),
            accept: clean_list(&it.accept),
            deps,
            // Which workflow builds it is the user's call, not the PM's — a
            // proposal isn't work anyone has agreed to do yet.
            workflow_def_id: None,
        });
    }
    Ok(out)
}

/// `roadmap_propose`: validate the batch, insert it as `proposed` rows in one
/// transaction, and hand back the allocated codes.
///
/// Returns the created rows alongside the response so the caller can announce
/// each one to the frontend — the board grows ghost rows live, mid-conversation.
fn propose_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Vec<RoadmapItem>) {
    let err = |msg: String| {
        (
            Response::err(id, format!("roadmap_propose: {msg}")),
            Vec::new(),
        )
    };

    let args: ProposeArgs = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let existing = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return err(e.to_string()),
    };
    let known: HashSet<&str> = existing.iter().map(|i| i.code.as_str()).collect();
    let news = match validate(&args.items, &known) {
        Ok(news) => news,
        Err(msg) => return err(msg),
    };

    // All or nothing: a failure half-way through must not leave the user
    // staring at three of the five tickets they were promised.
    let created = (|| -> rusqlite::Result<Vec<RoadmapItem>> {
        let tx = conn.unchecked_transaction()?;
        let mut created = Vec::with_capacity(news.len());
        for new in &news {
            created.push(store::create(&tx, project_id, new)?);
        }
        tx.commit()?;
        Ok(created)
    })();
    let created = match created {
        Ok(created) => created,
        Err(e) => return err(e.to_string()),
    };

    let payload = json!({
        "created": created
            .iter()
            .map(|i| json!({ "code": i.code, "title": i.title }))
            .collect::<Vec<_>>(),
    });
    match serde_json::to_string(&payload) {
        Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), created),
        // The rows exist either way — say so rather than implying nothing
        // happened, and still emit them.
        Err(e) => (
            Response::err(id, format!("roadmap_propose: created, but {e}")),
            created,
        ),
    }
}

// ───────────────────────────── dispatcher ───────────────────────────────

/// Adds the roadmap ops to a project-manager chat, over the standard git
/// dispatcher. Constructed in `supervisor::lifecycle` when a workspace's
/// `purpose` is `roadmap-pm`; no other agent is given one, which is why the ops
/// need no cap of their own.
pub struct RoadmapDispatcher {
    /// Where row changes are announced (`roadmap:item`), so the board follows a
    /// proposal live. `None` only in this module's tests, which have no window
    /// — the same shape `GitDispatcher::approval` uses.
    app: Option<AppHandle>,
    db: Db,
    /// The project this chat's board belongs to, stamped at spawn.
    project_id: String,
    git: GitDispatcher,
}

impl RoadmapDispatcher {
    pub fn new(app: AppHandle, db: Db, project_id: String, git: GitDispatcher) -> Self {
        Self {
            app: Some(app),
            db,
            project_id,
            git,
        }
    }
}

impl RpcDispatcher for RoadmapDispatcher {
    fn dispatch<'a>(
        &'a self,
        id: &'a str,
        op: &'a str,
        args: &'a Value,
    ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
        Box::pin(async move {
            if !is_roadmap_op(op) {
                // Everything else is the ordinary agent surface (echo, ping,
                // git_status, the credentialed git ops the advisory caps still
                // refuse). One git dispatcher, one refusal path.
                return self.git.dispatch(id, op, args).await;
            }
            match op {
                "roadmap_list" => {
                    let conn = self.db.lock();
                    (list_op(&conn, &self.project_id, id, args), Vec::new())
                }
                "roadmap_propose" => {
                    // Lock held only for the validate+insert; the emits happen
                    // after it is dropped.
                    let (resp, created) = {
                        let conn = self.db.lock();
                        propose_op(&conn, &self.project_id, id, args)
                    };
                    if let Some(app) = &self.app {
                        for item in &created {
                            crate::roadmap::emit_item(app, item);
                        }
                    }
                    (resp, Vec::new())
                }
                other => (
                    Response::err(
                        id,
                        format!(
                            "unknown roadmap op: {other} — this chat has {} and {}",
                            OPS[0], OPS[1]
                        ),
                    ),
                    Vec::new(),
                ),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::get_migrations;
    use crate::rpc::caps::AgentCaps;
    use parking_lot::Mutex;
    use rusqlite::params;
    use std::sync::Arc;

    /// A migrated in-memory DB with one project, matching how the store's own
    /// tests set up (the FK to `projects` is real).
    fn test_db(project_id: &str) -> Db {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        get_migrations().to_latest(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, created_at) VALUES (?1, 'my-cool-app', 0)",
            params![project_id],
        )
        .unwrap();
        Arc::new(Mutex::new(conn))
    }

    /// A dispatcher with no window to emit into — everything but the events.
    fn dispatcher(db: &Db, project_id: &str) -> RoadmapDispatcher {
        RoadmapDispatcher {
            app: None,
            db: db.clone(),
            project_id: project_id.to_string(),
            git: GitDispatcher::new(
                std::env::temp_dir(),
                "main".to_string(),
                AgentCaps::advisory(),
            ),
        }
    }

    /// The ops are synchronous under the lock, so most tests exercise them
    /// directly and only the routing tests go through `dispatch`.
    fn propose(db: &Db, args: Value) -> Response {
        let conn = db.lock();
        propose_op(&conn, "p1", "r1", &args).0
    }

    fn list(db: &Db, args: Value) -> Response {
        let conn = db.lock();
        list_op(&conn, "p1", "r1", &args)
    }

    fn one_item(title: &str) -> Value {
        json!({ "items": [{ "title": title, "why": "because", "horizon": "next" }] })
    }

    #[tokio::test]
    async fn propose_creates_proposed_pm_rows_and_returns_their_codes() {
        let db = test_db("p1");
        let d = dispatcher(&db, "p1");

        let resp = d
            .dispatch(
                "r1",
                "roadmap_propose",
                &json!({"items": [
                    {"title": "Ship the drainer", "why": "the queue needs one",
                     "horizon": "now", "size": "M", "area": "workflow",
                     "epic": "roadmap", "accept": ["it drains"]},
                    {"title": "Second", "why": "also", "horizon": "later"},
                ]}),
            )
            .await
            .0;
        assert!(resp.ok, "{resp:?}");

        let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
        let created = out["created"].as_array().unwrap();
        assert_eq!(created.len(), 2);
        // The allocated codes come back so the PM can name them in the chat.
        assert_eq!(created[0]["code"], "MCA-100");
        assert_eq!(created[1]["code"], "MCA-101");
        assert_eq!(created[0]["title"], "Ship the drainer");

        let rows = store::list(&db.lock(), "p1").unwrap();
        assert_eq!(rows.len(), 2);
        for row in &rows {
            // Nothing lands on the board unaccepted, and the board can tell who
            // wrote it.
            assert_eq!(row.status, ItemStatus::Proposed);
            assert_eq!(row.source, ItemSource::Pm);
        }
        assert_eq!(rows[0].horizon, Horizon::Now);
        assert_eq!(rows[0].size, Some(ItemSize::M));
        assert_eq!(rows[0].accept, vec!["it drains".to_string()]);
        assert_eq!(rows[0].epic.as_deref(), Some("roadmap"));
    }

    #[test]
    fn propose_rejects_the_whole_batch_on_any_bad_item() {
        let db = test_db("p1");

        // Empty batch.
        let resp = propose(&db, json!({ "items": [] }));
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("non-empty"));

        // No args at all is the same mistake.
        assert!(!propose(&db, Value::Null).ok);

        // A blank title, a bad horizon and a bad size each name the offending
        // item so the agent can fix exactly that one.
        for (args, needle) in [
            (
                json!({"items": [{"title": "ok", "horizon": "next"}, {"title": "  ", "horizon": "next"}]}),
                "item 2",
            ),
            (
                json!({"items": [{"title": "ok", "horizon": "soon"}]}),
                "unknown horizon",
            ),
            (json!({"items": [{"title": "ok"}]}), "`horizon` is required"),
            (
                json!({"items": [{"title": "ok", "horizon": "now", "size": "XXL"}]}),
                "unknown size",
            ),
            // A misspelled field would otherwise be silently dropped.
            (
                json!({"items": [{"title": "ok", "horizon": "now", "horizen": "next"}]}),
                "unknown field",
            ),
            // The agent may not decide an item is accepted.
            (
                json!({"items": [{"title": "ok", "horizon": "now", "status": "open"}]}),
                "unknown field",
            ),
        ] {
            let resp = propose(&db, args);
            assert!(!resp.ok, "should have been rejected");
            let e = resp.error.unwrap();
            assert!(e.contains(needle), "expected {needle:?} in {e:?}");
        }

        // Over the cap.
        let many: Vec<Value> = (0..MAX_BATCH + 1)
            .map(|n| json!({"title": format!("t{n}"), "horizon": "later"}))
            .collect();
        let resp = propose(&db, json!({ "items": many }));
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("too many"));

        // Not one row was written by any of the above.
        assert!(store::list(&db.lock(), "p1").unwrap().is_empty());
    }

    #[test]
    fn deps_must_name_codes_already_on_this_board() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("first")).ok);

        // A code from this project resolves.
        let resp = propose(
            &db,
            json!({"items": [{"title": "second", "horizon": "next", "deps": ["MCA-100"]}]}),
        );
        assert!(resp.ok, "{resp:?}");

        // One that doesn't exist rejects the batch, and says why.
        let resp = propose(
            &db,
            json!({"items": [{"title": "third", "horizon": "next", "deps": ["MCA-999"]}]}),
        );
        assert!(!resp.ok);
        let e = resp.error.unwrap();
        assert!(e.contains("MCA-999") && e.contains("roadmap_list"), "{e}");
        assert_eq!(store::list(&db.lock(), "p1").unwrap().len(), 2);
    }

    #[test]
    fn list_returns_the_project_board_and_filters_by_status() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("proposed one")).ok);
        // A shipped item: the PM must see what already landed.
        {
            let conn = db.lock();
            let done = store::create(
                &conn,
                "p1",
                &NewItem {
                    title: "shipped".into(),
                    status: Some(ItemStatus::Done),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(done.code, "MCA-101");
        }

        let resp = list(&db, Value::Null);
        assert!(resp.ok, "{resp:?}");
        let rows: Vec<Value> = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["code"], "MCA-100");
        assert_eq!(rows[0]["status"], "proposed");
        assert_eq!(rows[0]["why"], "because");
        assert_eq!(rows[1]["status"], "done");
        // Empties are omitted rather than sent as nulls.
        assert!(rows[1].get("why").is_none());
        assert!(rows[1].get("size").is_none());

        let resp = list(&db, json!({ "status": ["done"] }));
        let rows: Vec<Value> = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["code"], "MCA-101");

        let resp = list(&db, json!({ "status": ["shipped"] }));
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("unknown status"));
    }

    #[test]
    fn another_projects_board_is_invisible() {
        let db = test_db("p1");
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, created_at) VALUES ('p2', 'other', 0)",
                [],
            )
            .unwrap();
            store::create(
                &conn,
                "p2",
                &NewItem {
                    title: "theirs".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let resp = list(&db, Value::Null);
        assert_eq!(resp.stdout.unwrap(), "[]");
    }

    #[tokio::test]
    async fn unknown_roadmap_ops_are_named_and_everything_else_delegates() {
        let db = test_db("p1");
        let d = dispatcher(&db, "p1");

        let resp = d.dispatch("r1", "roadmap_delete", &Value::Null).await.0;
        assert!(!resp.ok);
        let e = resp.error.unwrap();
        assert!(
            e.contains("roadmap_list") && e.contains("roadmap_propose"),
            "{e}"
        );

        // A non-roadmap op falls through to the git dispatcher untouched.
        let resp = d.dispatch("r2", "ping", &Value::Null).await.0;
        assert!(resp.ok);
        assert_eq!(resp.stdout.unwrap(), "pong");

        // And the advisory grant still refuses to publish through it.
        let resp = d.dispatch("r3", "open_pr", &json!({})).await.0;
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("roadmap item"));
    }

    #[test]
    fn the_instruction_block_documents_exactly_these_ops() {
        // The PM only knows an op exists because the injected block says so; a
        // rename on either side is a tool the agent can't call.
        let block = crate::instructions::roadmap_block().expect("shipped default is non-empty");
        for op in OPS {
            assert!(block.contains(op), "instruction block never mentions {op}");
        }
        // And it must not promise anything this dispatcher would reject.
        for line in block.lines() {
            for word in line.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
                if word.starts_with("roadmap_") {
                    assert!(
                        OPS.contains(&word),
                        "block names an op we don't implement: {word}"
                    );
                }
            }
        }
    }
}
