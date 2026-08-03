//! The roadmap RPC ops: how the project-manager chat reads the board and puts
//! tickets on it.
//!
//! Same shape as [`crate::workflow::comms::WorkflowCommsDispatcher`]: this
//! dispatcher wraps the standard [`GitDispatcher`], owns the `roadmap_*` ops,
//! and delegates everything else — so the PM keeps the same read-only git
//! surface every other agent has (its `AgentCaps::advisory()` still refuses the
//! publish ops, one mechanism checked once).
//!
//! Four ops, all scoped to the project this chat belongs to. The project id is
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
//! - `roadmap_propose_update` / `roadmap_propose_discard` — the same contract
//!   for items that already exist: the ask lands as a pending delta
//!   ([`crate::roadmap::proposals`], at most one per item, a newer one
//!   replacing it) that only the user's ruling applies. The PM can reshape the
//!   board it argued for without ever holding the pen.
//!
//! Validation rejects the whole batch rather than creating a partial one: the
//! PM gets one precise error it can fix and retry, and the user never sees half
//! a proposal. The inserts run in a transaction for the same reason.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tauri::AppHandle;

use crate::roadmap::events::{self, EventActor, EventKind, ItemEvent};
use crate::roadmap::proposals::{self, Proposal, ProposalKind, ProposalPatch};
use crate::roadmap::store;
use crate::roadmap::types::{Horizon, ItemSource, ItemStatus, NewItem, RoadmapItem};
use crate::roadmap::Db;
use crate::rpc::git::GitDispatcher;
use crate::rpc::{Response, RpcDispatcher, RpcEvent, RpcFuture};

/// The ops this dispatcher owns. Pinned by a test against the instruction block
/// so the two can't drift — an agent told about an op that doesn't exist (or
/// given one it was never told about) is a silently broken tool.
pub const OPS: [&str; 4] = [
    "roadmap_list",
    "roadmap_propose",
    "roadmap_propose_update",
    "roadmap_propose_discard",
];

/// Most items one `roadmap_propose` call may carry. A proposal is a thing a
/// human reads and accepts; past a score of rows that stops being true, and the
/// PM should be slicing rather than dumping a backlog.
const MAX_BATCH: usize = 20;

/// Is `op` one this dispatcher owns? The whole `roadmap_` namespace, not just
/// the known names, so a typo'd op gets a precise error naming the real ones
/// instead of the git dispatcher's generic "unknown op".
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
    area: Option<String>,
    #[serde(default)]
    accept: Vec<String>,
    #[serde(default)]
    deps: Vec<String>,
}

/// `roadmap_propose_update` args. `patch` is the [`ProposalPatch`] shape —
/// `deny_unknown_fields` there is what refuses `status`/`code`/`source` and
/// every run back-link with a precise error.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeUpdateArgs {
    code: String,
    patch: ProposalPatch,
    /// One honest sentence on why — quoted on the card next to the diff.
    #[serde(default)]
    note: Option<String>,
}

/// `roadmap_propose_discard` args. Unlike the update's optional note, the
/// `reason` is required: asking to remove work someone agreed to is exactly
/// the ask that must explain itself.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeDiscardArgs {
    code: String,
    #[serde(default)]
    reason: String,
}

/// Decode `args` into an op's shape. A missing `args` arrives as JSON null,
/// which is the same as `{}` for every op here.
fn parse_args<T: Default + serde::de::DeserializeOwned>(args: &Value) -> Result<T, String> {
    if args.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(args.clone()).map_err(|e| e.to_string())
}

/// Decode `args` for an op with required fields, where a missing `args` is a
/// mistake worth naming rather than an empty default.
fn parse_required<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T, String> {
    if args.is_null() {
        return Err("`args` are required".into());
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
///
/// `pending` is the item's outstanding delta, if any, summarized as
/// `pending_proposal` — so the PM knows what it has already asked for and
/// never re-proposes blind (or mistakes "not applied yet" for "declined").
fn compact(item: &RoadmapItem, pending: Option<&Proposal>) -> Value {
    let mut o = Map::new();
    o.insert("code".into(), json!(item.code));
    o.insert("title".into(), json!(item.title));
    o.insert("horizon".into(), json!(item.horizon.as_str()));
    o.insert("status".into(), json!(item.status.as_str()));
    if !item.why.is_empty() {
        o.insert("why".into(), json!(item.why));
    }
    if let Some(area) = &item.area {
        o.insert("area".into(), json!(area));
    }
    if !item.accept.is_empty() {
        o.insert("accept".into(), json!(item.accept));
    }
    if !item.deps.is_empty() {
        o.insert("deps".into(), json!(item.deps));
    }
    // Quoted only while the item can still be ruled: an ask whose item has
    // advanced past the gate has no card to rule it from, and quoting it
    // forever would read as "still waiting on the user" when nothing is.
    if let Some(p) = pending.filter(|_| rulable(item.status)) {
        let mut pp = Map::new();
        pp.insert("kind".into(), json!(p.kind.as_str()));
        if let Some(note) = &p.note {
            pp.insert("note".into(), json!(note));
        }
        // The changed fields, for an update; a discard carries none.
        let fields = p.fields();
        if !fields.is_empty() {
            pp.insert("fields".into(), json!(fields));
        }
        o.insert("pending_proposal".into(), Value::Object(pp));
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
    let pending = match proposals::list_for_project(conn, project_id) {
        Ok(list) => list,
        Err(e) => return Response::err(id, format!("roadmap_list: {e}")),
    };
    let by_item: HashMap<&str, &Proposal> =
        pending.iter().map(|p| (p.item_id.as_str(), p)).collect();
    let keep = |i: &RoadmapItem| match &filter {
        None => true,
        Some(f) => f.contains(i.status.as_str()),
    };
    let rows: Vec<Value> = items
        .iter()
        .filter(|i| keep(i))
        .map(|i| compact(i, by_item.get(i.id.as_str()).copied()))
        .collect();
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
            area: clean(it.area.as_deref()),
            source: Some(ItemSource::Pm),
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
/// Returns the created rows — and the `proposed` history events recorded with
/// them — alongside the response so the caller can announce both to the
/// frontend: the board grows ghost rows live, mid-conversation.
fn propose_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Vec<RoadmapItem>, Vec<ItemEvent>) {
    let err = |msg: String| {
        (
            Response::err(id, format!("roadmap_propose: {msg}")),
            Vec::new(),
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
    // staring at three of the five tickets they were promised. Each row's
    // `proposed` history event rides the same transaction, so a ghost can never
    // exist without the record of who suggested it.
    let created = (|| -> rusqlite::Result<(Vec<RoadmapItem>, Vec<ItemEvent>)> {
        let tx = conn.unchecked_transaction()?;
        let mut created = Vec::with_capacity(news.len());
        let mut recorded = Vec::with_capacity(news.len());
        for new in &news {
            let item = store::create(&tx, project_id, new)?;
            recorded.push(events::record(
                &tx,
                &item.id,
                project_id,
                EventActor::Pm,
                EventKind::Proposed,
                None,
            )?);
            created.push(item);
        }
        tx.commit()?;
        Ok((created, recorded))
    })();
    let (created, recorded) = match created {
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
        Ok(stdout) => (
            Response::ok(id, 0, stdout, String::new()),
            created,
            recorded,
        ),
        // The rows exist either way — say so rather than implying nothing
        // happened, and still emit them.
        Err(e) => (
            Response::err(id, format!("roadmap_propose: created, but {e}")),
            created,
            recorded,
        ),
    }
}

// ──────────────── roadmap_propose_update / _discard ─────────────────────

/// Find the item an ask targets and check it may still be reshaped. Anything
/// from `active` on belongs to its run: a proposal against it would be ruled
/// on against work that no longer matches the diff.
fn proposable<'a>(items: &'a [RoadmapItem], code: &str) -> Result<&'a RoadmapItem, String> {
    let item = items.iter().find(|i| i.code == code).ok_or_else(|| {
        format!("no item {code:?} on this board — `roadmap_list` shows what exists")
    })?;
    if rulable(item.status) {
        Ok(item)
    } else {
        Err(format!(
            "{} is {} — an item being built or reviewed can't be reshaped by proposal; \
             use the codes `roadmap_list` shows as proposed, open, or queued",
            item.code,
            item.status.as_str()
        ))
    }
}

/// May an ask against an item with this status still be ruled on? Anything
/// from `active` on is being built or judged. Shared by the propose-time gate
/// above and the `compact` projection, so the PM is never quoted an ask the
/// user has no card to rule. (The ruling-side copy of this set lives in
/// `roadmap::proposal_gate`; unifying them is filed for B5.)
fn rulable(status: ItemStatus) -> bool {
    matches!(
        status,
        ItemStatus::Proposed | ItemStatus::Open | ItemStatus::Queued
    )
}

/// Normalize and validate an update's patch against the board, or say exactly
/// what's wrong: same rules the batch propose applies, plus "don't depend on
/// yourself" (possible here because the target already has a code).
fn validate_patch(
    patch: &ProposalPatch,
    item: &RoadmapItem,
    known: &HashSet<&str>,
) -> Result<ProposalPatch, String> {
    if patch.is_empty() {
        return Err("`patch` must change at least one field — \
                    title | why | horizon | area | accept | deps"
            .into());
    }
    let mut out = patch.clone();
    if let Some(title) = &out.title {
        let title = title.trim();
        if title.is_empty() {
            return Err("`title` cannot be blank".into());
        }
        out.title = Some(title.to_string());
    }
    if let Some(why) = &out.why {
        out.why = Some(why.trim().to_string());
    }
    if let Some(Some(area)) = &out.area {
        let area = area.trim();
        if area.is_empty() {
            return Err("`area` cannot be blank — send `\"area\": null` to clear it".into());
        }
        out.area = Some(Some(area.to_string()));
    }
    if let Some(accept) = &out.accept {
        out.accept = Some(clean_list(accept));
    }
    if let Some(deps) = &out.deps {
        let deps = clean_list(deps);
        for d in &deps {
            if d == &item.code {
                return Err(format!("{} cannot depend on itself", item.code));
            }
            if !known.contains(d.as_str()) {
                return Err(format!(
                    "`deps` names {d:?}, which is not an item on this board — \
                     depend only on codes `roadmap_list` returns"
                ));
            }
        }
        out.deps = Some(deps);
    }
    Ok(out)
}

/// `roadmap_propose_update`: park a validated patch as the item's pending
/// delta, replacing any it already has. Nothing is applied here — the user's
/// ruling does that — so there is no history event either: the ruling writes
/// history, not the ask.
///
/// Returns the stored proposal alongside the response so the dispatcher can
/// announce it (`roadmap:proposal`) after the lock drops.
fn propose_update_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<Proposal>) {
    let err = |msg: String| {
        (
            Response::err(id, format!("roadmap_propose_update: {msg}")),
            None,
        )
    };
    let args: ProposeUpdateArgs = match parse_required(args) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return err(e.to_string()),
    };
    let item = match proposable(&items, args.code.trim()) {
        Ok(item) => item,
        Err(e) => return err(e),
    };
    let known: HashSet<&str> = items.iter().map(|i| i.code.as_str()).collect();
    let patch = match validate_patch(&args.patch, item, &known) {
        Ok(patch) => patch,
        Err(e) => return err(e),
    };
    let note = clean(args.note.as_deref());
    let stored = match proposals::upsert(
        conn,
        project_id,
        &item.id,
        ProposalKind::Update,
        Some(&patch),
        note.as_deref(),
    ) {
        Ok(p) => p,
        Err(e) => return err(e.to_string()),
    };

    let payload = json!({ "proposed": { "code": item.code, "fields": patch.fields() } });
    match serde_json::to_string(&payload) {
        Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), Some(stored)),
        Err(e) => err(e.to_string()),
    }
}

/// `roadmap_propose_discard`: park a removal ask as the item's pending delta.
/// Same shape as the update — nothing is deleted until the user rules.
fn propose_discard_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<Proposal>) {
    let err = |msg: String| {
        (
            Response::err(id, format!("roadmap_propose_discard: {msg}")),
            None,
        )
    };
    let args: ProposeDiscardArgs = match parse_required(args) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let reason = args.reason.trim();
    if reason.is_empty() {
        return err("`reason` is required — say why this should leave the board".into());
    }
    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return err(e.to_string()),
    };
    let item = match proposable(&items, args.code.trim()) {
        Ok(item) => item,
        Err(e) => return err(e),
    };
    let stored = match proposals::upsert(
        conn,
        project_id,
        &item.id,
        ProposalKind::Discard,
        None,
        Some(reason),
    ) {
        Ok(p) => p,
        Err(e) => return err(e.to_string()),
    };

    let payload = json!({ "proposed": { "code": item.code, "kind": "discard" } });
    match serde_json::to_string(&payload) {
        Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), Some(stored)),
        Err(e) => err(e.to_string()),
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
                    let (resp, created, recorded) = {
                        let conn = self.db.lock();
                        propose_op(&conn, &self.project_id, id, args)
                    };
                    if let Some(app) = &self.app {
                        for item in &created {
                            crate::roadmap::emit_item(app, item);
                        }
                        for event in &recorded {
                            crate::roadmap::emit_item_event(app, event);
                        }
                    }
                    (resp, Vec::new())
                }
                "roadmap_propose_update" | "roadmap_propose_discard" => {
                    // Same lock discipline as the batch propose: validate and
                    // store under the lock, announce after it drops. No item
                    // event here — the user's ruling writes the history.
                    let (resp, stored) = {
                        let conn = self.db.lock();
                        if op == "roadmap_propose_update" {
                            propose_update_op(&conn, &self.project_id, id, args)
                        } else {
                            propose_discard_op(&conn, &self.project_id, id, args)
                        }
                    };
                    if let (Some(app), Some(p)) = (&self.app, &stored) {
                        crate::roadmap::emit_proposal(app, p);
                    }
                    (resp, Vec::new())
                }
                other => (
                    Response::err(
                        id,
                        format!(
                            "unknown roadmap op: {other} — this chat has {}",
                            OPS.join(", ")
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

    fn propose_update(db: &Db, args: Value) -> (Response, Option<Proposal>) {
        let conn = db.lock();
        propose_update_op(&conn, "p1", "r1", &args)
    }

    fn propose_discard(db: &Db, args: Value) -> (Response, Option<Proposal>) {
        let conn = db.lock();
        propose_discard_op(&conn, "p1", "r1", &args)
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
                     "horizon": "now", "area": "workflow", "accept": ["it drains"]},
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
            // Each proposal starts its durable history: one `proposed` event,
            // attributed to the PM.
            let history = events::list_for_item(&db.lock(), &row.id).unwrap();
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].kind, EventKind::Proposed);
            assert_eq!(history[0].actor, EventActor::Pm);
        }
        assert_eq!(rows[0].horizon, Horizon::Now);
        assert_eq!(rows[0].accept, vec!["it drains".to_string()]);
        assert_eq!(rows[0].area.as_deref(), Some("workflow"));
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

        // A blank title and a bad horizon each name the offending item so the
        // agent can fix exactly that one.
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
            // The pruned fields are gone, not ignored: a PM still sending them
            // gets a precise refusal (see .context/roadmap-pm-plan.md, A0).
            (
                json!({"items": [{"title": "ok", "horizon": "now", "size": "M"}]}),
                "unknown field",
            ),
            (
                json!({"items": [{"title": "ok", "horizon": "now", "epic": "roadmap"}]}),
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
        assert!(rows[1].get("area").is_none());

        let resp = list(&db, json!({ "status": ["done"] }));
        let rows: Vec<Value> = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["code"], "MCA-101");

        let resp = list(&db, json!({ "status": ["shipped"] }));
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("unknown status"));
    }

    #[test]
    fn propose_update_parks_a_delta_and_the_listing_shows_it() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("target")).ok); // MCA-100
        assert!(propose(&db, one_item("dep")).ok); // MCA-101

        let (resp, stored) = propose_update(
            &db,
            json!({"code": "MCA-100", "note": "scope grew",
                   "patch": {"title": "Retitled", "deps": ["MCA-101"]}}),
        );
        assert!(resp.ok, "{resp:?}");
        let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
        // The response quotes what was asked, so the PM can say it in the chat.
        assert_eq!(out["proposed"]["code"], "MCA-100");
        assert_eq!(out["proposed"]["fields"], json!(["title", "deps"]));

        // Parked, not applied: the row is untouched until the user rules.
        let rows = store::list(&db.lock(), "p1").unwrap();
        assert_eq!(rows[0].title, "target");
        let p = stored.unwrap();
        assert_eq!(p.kind, ProposalKind::Update);
        assert_eq!(p.note.as_deref(), Some("scope grew"));
        // And no history either — the ruling writes history, not the ask.
        assert!(events::list_for_item(&db.lock(), &rows[0].id)
            .unwrap()
            .iter()
            .all(|e| e.kind == EventKind::Proposed));

        // The compact listing carries the pending ask, so the PM never
        // re-proposes blind.
        let resp = list(&db, Value::Null);
        let listed: Vec<Value> = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
        let pp = &listed[0]["pending_proposal"];
        assert_eq!(pp["kind"], "update");
        assert_eq!(pp["note"], "scope grew");
        assert_eq!(pp["fields"], json!(["title", "deps"]));
        assert!(listed[1].get("pending_proposal").is_none());
    }

    #[test]
    fn propose_update_rejects_bad_asks_precisely() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("target")).ok); // MCA-100
        {
            // An item already being built — not reshapeable by proposal.
            let conn = db.lock();
            store::create(
                &conn,
                "p1",
                &NewItem {
                    title: "building".into(),
                    status: Some(ItemStatus::Active),
                    ..Default::default()
                },
            )
            .unwrap(); // MCA-101
        }

        for (args, needle) in [
            // The lifecycle is not the PM's to move, even by proposal.
            (
                json!({"code": "MCA-100", "patch": {"status": "open"}}),
                "unknown field",
            ),
            (
                json!({"code": "MCA-100", "patch": {"horizen": "next"}}),
                "unknown field",
            ),
            (
                json!({"code": "MCA-100", "patch": {"horizon": "soon"}}),
                "invalid Horizon",
            ),
            (
                json!({"code": "MCA-100", "patch": {"deps": ["MCA-999"]}}),
                "MCA-999",
            ),
            (
                json!({"code": "MCA-100", "patch": {"deps": ["MCA-100"]}}),
                "depend on itself",
            ),
            (
                json!({"code": "MCA-100", "patch": {"title": "  "}}),
                "cannot be blank",
            ),
            (
                json!({"code": "MCA-100", "patch": {}}),
                "at least one field",
            ),
            (
                json!({"code": "MCA-777", "patch": {"title": "x"}}),
                "no item",
            ),
            // The refusal names the status, so the PM knows why and when.
            (
                json!({"code": "MCA-101", "patch": {"title": "x"}}),
                "MCA-101 is active",
            ),
        ] {
            let (resp, stored) = propose_update(&db, args);
            assert!(!resp.ok, "should have been rejected");
            let e = resp.error.unwrap();
            assert!(e.contains(needle), "expected {needle:?} in {e:?}");
            assert!(stored.is_none());
        }
        // None of the above parked anything.
        assert!(proposals::list_for_project(&db.lock(), "p1")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_newer_ask_replaces_the_pending_one() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("target")).ok); // MCA-100

        let (first, _) =
            propose_update(&db, json!({"code": "MCA-100", "patch": {"title": "first"}}));
        assert!(first.ok);
        let (second, stored) = propose_discard(
            &db,
            json!({"code": "MCA-100", "reason": "superseded by the auth slice"}),
        );
        assert!(second.ok, "{second:?}");

        // One pending ask per item: the discard replaced the retitle.
        let pending = proposals::list_for_project(&db.lock(), "p1").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0], stored.unwrap());
        assert_eq!(pending[0].kind, ProposalKind::Discard);
        assert_eq!(pending[0].patch, None);
    }

    #[test]
    fn propose_discard_requires_a_reason() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("target")).ok); // MCA-100

        let (resp, stored) = propose_discard(&db, json!({"code": "MCA-100", "reason": "  "}));
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("`reason` is required"));
        assert!(stored.is_none());

        // And args at all, for both ops.
        assert!(!propose_discard(&db, Value::Null).0.ok);
        assert!(!propose_update(&db, Value::Null).0.ok);
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
