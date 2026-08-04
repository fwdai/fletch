//! The roadmap RPC ops: how the project-manager chat reads the board and puts
//! tickets on it.
//!
//! Same shape as [`crate::workflow::comms::WorkflowCommsDispatcher`]: this
//! dispatcher wraps the standard [`GitDispatcher`], owns the `roadmap_*` ops,
//! and delegates everything else — so the PM keeps the same read-only git
//! surface every other agent has (its `AgentCaps::advisory()` still refuses the
//! publish ops, one mechanism checked once).
//!
//! Six ops, all scoped to the project this chat belongs to. The project id is
//! stamped at construction from the workspace record, never taken from `args`:
//! a chat can only ever read and write its own project's board.
//!
//! - `roadmap_list` — the whole board, compact, in board order (which is rank
//!   order, i.e. dispatch order), including `done` items (the PM needs to know
//!   what already shipped before it proposes more). Each row carries its
//!   `last_event` and its PR link when it has one, so "why did MCA-104 fail?"
//!   is answerable from this one call — the PM oversees execution, not just
//!   intake.
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
//! - `roadmap_propose_order` — the same contract for the board's *order*: a
//!   whole-board ask ([`crate::roadmap::order`]) naming every orderable item in
//!   the sequence the PM argues for. Board scoped rather than item scoped, and
//!   refused unless it covers the orderable set exactly, so what the user rules
//!   on is unambiguous.
//! - `roadmap_note` — the one op that writes *directly*, because it advances
//!   nothing: a durable `note` on the item's history. Attention, not action.
//!   That is the whole of the PM's direct-write licence (invariant 2 in
//!   .context/roadmap-pm-plan.md): it may raise a hand, never move a piece.
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
use crate::roadmap::order::{self, OrderProposal};
use crate::roadmap::proposals::{self, Proposal, ProposalKind, ProposalPatch};
use crate::roadmap::store;
use crate::roadmap::types::{Horizon, ItemSource, ItemStatus, NewItem, RoadmapItem};
use crate::roadmap::Db;
use crate::rpc::git::GitDispatcher;
use crate::rpc::{Response, RpcDispatcher, RpcEvent, RpcFuture};

/// The ops this dispatcher owns. Pinned by a test against the instruction block
/// so the two can't drift — an agent told about an op that doesn't exist (or
/// given one it was never told about) is a silently broken tool.
pub const OPS: [&str; 6] = [
    "roadmap_list",
    "roadmap_propose",
    "roadmap_propose_update",
    "roadmap_propose_discard",
    "roadmap_propose_order",
    "roadmap_note",
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

/// `roadmap_propose_order` args: the whole new sequence, and why. `codes` must
/// name every orderable item on the board — the refusal says which are missing
/// or don't belong (see [`order::validate_order`]).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeOrderArgs {
    #[serde(default)]
    codes: Vec<String>,
    /// One honest sentence on why this order — the user reads it above the board.
    #[serde(default)]
    note: Option<String>,
}

/// `roadmap_note` args: which item, and the observation. Both required — a note
/// with no text is the one thing this op cannot record.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteArgs {
    code: String,
    #[serde(default)]
    note: String,
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
///
/// `last` is the item's newest history row, projected as `last_event`. This is
/// what turns the listing from an intake queue into an execution report: the
/// status says *where* an item is, the last event says *what happened* — a
/// failure reason, a workflow, a note somebody left. `age` is relative on
/// purpose, computed against `now`: an absolute epoch means nothing to an agent
/// reasoning about "since we last spoke", and a wall-clock timestamp it would
/// have to diff itself is a round trip and a mistake waiting to happen.
///
/// The PR link rides along as `pr` for the same reason — the diff is where a
/// review actually happens, and the item's own `status` already says whether
/// that PR is still open (`in_review`) or landed (`done`), so no polled state is
/// invented here. Raw ids stay hidden throughout: run ids, item ids and PR
/// numbers are the app's handles, not the PM's vocabulary.
fn compact(
    item: &RoadmapItem,
    pending: Option<&Proposal>,
    last: Option<&ItemEvent>,
    now: i64,
) -> Value {
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
    if let Some(e) = last {
        let mut le = Map::new();
        le.insert("kind".into(), json!(e.kind.as_str()));
        if let Some(detail) = &e.detail {
            le.insert("detail".into(), json!(detail));
        }
        if let Some(age) = age(now, e.created_at) {
            le.insert("age".into(), json!(age));
        }
        o.insert("last_event".into(), Value::Object(le));
    }
    if let Some(url) = item
        .pr_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        o.insert("pr".into(), json!({ "url": url }));
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

/// How long ago something happened, in the coarsest unit that is still true:
/// `"4m"`, `"2h"`, `"3d"`. `None` for anything under a minute (and for a clock
/// that ran backwards) — "just now" is what the absence means, and inventing
/// `"0m"` would read as staler than it is.
///
/// Coarse deliberately: the PM reasons in "since we last spoke", and a precise
/// duration would invite arithmetic it has no reason to do.
fn age(now: i64, then: i64) -> Option<String> {
    let ms = now.checked_sub(then).filter(|d| *d > 0)?;
    let minutes = ms / 60_000;
    match minutes {
        0 => None,
        m if m < 60 => Some(format!("{m}m")),
        m if m < 60 * 24 => Some(format!("{}h", m / 60)),
        m => Some(format!("{}d", m / (60 * 24))),
    }
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
    // One statement for the whole board rather than a query per row: the PM
    // reads this listing constantly, and it is the only thing between it and
    // knowing what the runs did.
    let last = match events::latest_by_item(conn, project_id) {
        Ok(map) => map,
        Err(e) => return Response::err(id, format!("roadmap_list: {e}")),
    };
    let now = crate::database::now_millis();
    let keep = |i: &RoadmapItem| match &filter {
        None => true,
        Some(f) => f.contains(i.status.as_str()),
    };
    let rows: Vec<Value> = items
        .iter()
        .filter(|i| keep(i))
        .map(|i| compact(i, by_item.get(i.id.as_str()).copied(), last.get(&i.id), now))
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

// ───────────────────────────── roadmap_note ─────────────────────────────

/// Longest note this op will store. A note is a line on a card and a line in the
/// PM's next listing — past a couple of sentences it stops being an observation
/// and starts being an essay nobody reads, and the thing it should have been is
/// a proposal.
const MAX_NOTE: usize = 500;

/// `roadmap_note`: record one durable observation on an item.
///
/// The PM's only direct write, and it is allowed precisely because it advances
/// nothing: no status moves, no field changes, no queue is touched. It raises
/// attention — the conservative direction of invariant 2 — where every ask that
/// would *do* something stays a proposal the user rules on.
///
/// Unlike the propose ops, the target may be at **any** status. The observation
/// worth recording most often concerns an item that is already `active`,
/// `in_review` or `done` ("this shipped, but it solved a narrower problem than
/// MCA-104 asked for"), and that is exactly the item a proposal is refused on.
/// Refusing the note too would leave the PM with nowhere to put the one thing it
/// is uniquely positioned to notice.
///
/// Returns the recorded event alongside the response so the dispatcher can
/// announce it (`roadmap:item-event`) once the lock drops — the card's trail
/// grows mid-conversation.
fn note_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<ItemEvent>) {
    let err = |msg: String| (Response::err(id, format!("roadmap_note: {msg}")), None);
    let args: NoteArgs = match parse_required(args) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let note = args.note.trim();
    if note.is_empty() {
        return err("`note` is required — say what you observed, in one honest sentence".into());
    }
    // Counted in characters, not bytes: the cap is about how much a human will
    // read, and a byte limit would refuse a shorter note for containing an
    // em-dash.
    let length = note.chars().count();
    if length > MAX_NOTE {
        return err(format!(
            "`note` is {length} characters — keep it under {MAX_NOTE}. A note is one observation; \
             if it needs more than that, it is a proposal"
        ));
    }
    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return err(e.to_string()),
    };
    let code = args.code.trim();
    // Any status: see the doc comment. Only "no such item" is refused.
    let Some(item) = items.iter().find(|i| i.code == code) else {
        return err(format!(
            "no item {code:?} on this board — `roadmap_list` shows what exists"
        ));
    };
    let recorded = events::record(
        conn,
        &item.id,
        project_id,
        EventActor::Pm,
        EventKind::Note,
        Some(note),
    );
    let event = match recorded {
        Ok(event) => event,
        Err(e) => return err(e.to_string()),
    };

    let payload = json!({ "noted": { "code": item.code } });
    match serde_json::to_string(&payload) {
        Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), Some(event)),
        // The note is on the card either way — say so rather than implying
        // nothing happened, and still announce it.
        Err(e) => (
            Response::err(id, format!("roadmap_note: recorded, but {e}")),
            Some(event),
        ),
    }
}

// ───────────────────────── roadmap_propose_order ────────────────────────

/// `roadmap_propose_order`: park a whole-board order ask, replacing any the
/// project already has.
///
/// The sequence must be *exactly* the board's orderable set — refused otherwise,
/// naming what's missing or what doesn't belong. That is what makes the ask mean
/// one thing: it IS the new backlog order, not a hint about part of one, so the
/// user can rule on it without reconstructing where the unnamed items went.
/// Nothing is applied here; the ruling rewrites the ranks.
fn propose_order_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<OrderProposal>) {
    let err = |msg: String| {
        (
            Response::err(id, format!("roadmap_propose_order: {msg}")),
            None,
        )
    };
    let args: ProposeOrderArgs = match parse_required(args) {
        Ok(a) => a,
        Err(e) => return err(e),
    };
    let items = match store::list(conn, project_id) {
        Ok(items) => items,
        Err(e) => return err(e.to_string()),
    };
    let codes = clean_list(&args.codes);
    // Validated here *and* at ruling time, against the same function: the board
    // moves while an ask is pending, and the user's click must not apply a
    // sequence that no longer covers it.
    if let Err(e) = order::validate_order(&codes, &items) {
        return err(e);
    }
    let note = clean(args.note.as_deref());
    let stored = match order::upsert(conn, project_id, &codes, note.as_deref()) {
        Ok(p) => p,
        Err(e) => return err(e.to_string()),
    };

    let payload = json!({ "proposed": { "order": codes } });
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
                "roadmap_propose_order" => {
                    // Same lock discipline again; the ask is board-scoped, so
                    // what is announced is one row keyed by project.
                    let (resp, stored) = {
                        let conn = self.db.lock();
                        propose_order_op(&conn, &self.project_id, id, args)
                    };
                    if let (Some(app), Some(p)) = (&self.app, &stored) {
                        crate::roadmap::emit_order_proposal(app, p);
                    }
                    (resp, Vec::new())
                }
                "roadmap_note" => {
                    // The one op that writes directly. Same lock discipline all
                    // the same: the event lands under the lock, and the card
                    // hears about it after the guard drops.
                    let (resp, recorded) = {
                        let conn = self.db.lock();
                        note_op(&conn, &self.project_id, id, args)
                    };
                    if let (Some(app), Some(event)) = (&self.app, &recorded) {
                        crate::roadmap::emit_item_event(app, event);
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

    fn propose_order(db: &Db, args: Value) -> (Response, Option<OrderProposal>) {
        let conn = db.lock();
        propose_order_op(&conn, "p1", "r1", &args)
    }

    fn note(db: &Db, args: Value) -> (Response, Option<ItemEvent>) {
        let conn = db.lock();
        note_op(&conn, "p1", "r1", &args)
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

    /// The listing is the PM's execution report, not just its intake queue: each
    /// row carries the newest thing that happened to it, and the PR link when
    /// there is one — enough to answer "why did MCA-101 fail?" from this one
    /// call, with no ids leaked.
    #[test]
    fn the_listing_carries_each_items_last_event_and_pr() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("proposed one")).ok); // MCA-100
        let failed = {
            let conn = db.lock();
            let it = store::create(
                &conn,
                "p1",
                &NewItem {
                    title: "failed one".into(),
                    status: Some(ItemStatus::Open),
                    ..Default::default()
                },
            )
            .unwrap(); // MCA-101
            events::record(
                &conn,
                &it.id,
                "p1",
                EventActor::Drainer,
                EventKind::RunFailed,
                Some("its run failed"),
            )
            .unwrap();
            it
        };
        // An item in review, with the PR the run opened stamped on it.
        {
            let conn = db.lock();
            let it = store::create(
                &conn,
                "p1",
                &NewItem {
                    title: "in review".into(),
                    status: Some(ItemStatus::InReview),
                    ..Default::default()
                },
            )
            .unwrap(); // MCA-102
            store::update(
                &conn,
                &it.id,
                &crate::roadmap::types::ItemPatch {
                    pr_url: Some(Some("https://github.com/o/r/pull/7".into())),
                    ..Default::default()
                },
            )
            .unwrap();
            events::record(
                &conn,
                &it.id,
                "p1",
                EventActor::Drainer,
                EventKind::PrOpened,
                Some("https://github.com/o/r/pull/7"),
            )
            .unwrap();
        }

        let resp = list(&db, Value::Null);
        let rows: Vec<Value> = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
        assert_eq!(rows[0]["last_event"]["kind"], "proposed");
        // No detail, no age (it happened this millisecond) — the keys are simply
        // absent rather than null.
        assert!(rows[0]["last_event"].get("detail").is_none());
        assert!(rows[0]["last_event"].get("age").is_none());
        assert!(rows[0].get("pr").is_none());

        assert_eq!(rows[1]["code"], failed.code);
        assert_eq!(rows[1]["last_event"]["kind"], "run_failed");
        assert_eq!(rows[1]["last_event"]["detail"], "its run failed");

        assert_eq!(rows[2]["last_event"]["kind"], "pr_opened");
        assert_eq!(rows[2]["pr"]["url"], "https://github.com/o/r/pull/7");
        // The PR's *number* is an app handle, not the PM's vocabulary.
        assert!(rows[2]["pr"].get("number").is_none());

        // An item with no history at all simply has no `last_event`.
        {
            let conn = db.lock();
            store::create(
                &conn,
                "p1",
                &NewItem {
                    title: "silent".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let resp = list(&db, Value::Null);
        let rows: Vec<Value> = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
        assert!(rows[3].get("last_event").is_none());
    }

    /// `age` is coarse and relative, because "since we last spoke" is the only
    /// question the PM asks of it. Under a minute is absent — "just now".
    #[test]
    fn the_age_of_an_event_reads_in_the_coarsest_true_unit() {
        let now = 1_000_000_000_000;
        let min = 60_000;
        for (ago, expected) in [
            (0, None),
            (min - 1, None),
            (min, Some("1m")),
            (59 * min, Some("59m")),
            (60 * min, Some("1h")),
            (23 * 60 * min + 59 * min, Some("23h")),
            (24 * 60 * min, Some("1d")),
            (9 * 24 * 60 * min, Some("9d")),
        ] {
            assert_eq!(age(now, now - ago).as_deref(), expected, "{ago}ms ago");
        }
        // A clock that ran backwards reads as "just now" rather than negative.
        assert_eq!(age(now, now + 5 * min), None);
    }

    /// `roadmap_note` writes one durable `note` event attributed to the PM, and
    /// changes nothing else about the item — the whole of the direct-write
    /// licence.
    #[test]
    fn note_records_an_observation_and_advances_nothing() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("target")).ok); // MCA-100
        let before = store::list(&db.lock(), "p1").unwrap();

        let (resp, event) = note(
            &db,
            json!({"code": "MCA-100", "note": "the run solved a narrower problem than this asked for"}),
        );
        assert!(resp.ok, "{resp:?}");
        let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
        assert_eq!(out["noted"]["code"], "MCA-100");

        let event = event.expect("the note is returned so the card can hear about it");
        assert_eq!(event.kind, EventKind::Note);
        assert_eq!(event.actor, EventActor::Pm);
        assert_eq!(
            event.detail.as_deref(),
            Some("the run solved a narrower problem than this asked for")
        );
        assert_eq!(event.item_id, before[0].id);
        assert_eq!(event.project_id, "p1");

        // The row itself is byte-for-byte what it was: a note is attention, not
        // action.
        assert_eq!(store::list(&db.lock(), "p1").unwrap(), before);
        // And it is on the trail, on top of the `proposed` that opened it.
        let trail = events::list_for_item(&db.lock(), &before[0].id).unwrap();
        assert_eq!(trail.len(), 2);
        assert_eq!(trail[0], event);
    }

    /// A note is the one PM write allowed on an item a proposal is refused on —
    /// `active`, `in_review`, `done`. That item is usually the whole reason the
    /// op exists.
    #[test]
    fn note_lands_on_items_no_proposal_could_touch() {
        let db = test_db("p1");
        for status in [ItemStatus::Active, ItemStatus::InReview, ItemStatus::Done] {
            let it = {
                let conn = db.lock();
                store::create(
                    &conn,
                    "p1",
                    &NewItem {
                        title: "shipped something".into(),
                        status: Some(status),
                        ..Default::default()
                    },
                )
                .unwrap()
            };
            let (resp, event) = note(
                &db,
                json!({"code": it.code, "note": "narrower than agreed"}),
            );
            assert!(
                resp.ok,
                "a note on {} must be allowed: {resp:?}",
                status.as_str()
            );
            assert_eq!(event.unwrap().item_id, it.id);
            // The proposal path still refuses it — the two gates say different
            // things on purpose.
            let items = store::list(&db.lock(), "p1").unwrap();
            assert!(proposable(&items, &it.code).is_err());
        }
    }

    /// Every way a note can be wrong, named precisely, writing nothing.
    #[test]
    fn note_rejects_bad_asks_precisely() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("target")).ok); // MCA-100
        let long = "x".repeat(MAX_NOTE + 1);

        for (args, needle) in [
            (json!({"code": "MCA-777", "note": "hi"}), "no item"),
            (
                json!({"code": "MCA-100", "note": "   "}),
                "`note` is required",
            ),
            (json!({"code": "MCA-100"}), "`note` is required"),
            (
                json!({"code": "MCA-100", "note": long.clone()}),
                "keep it under",
            ),
            // The note is not a back door to the fields the propose ops gate.
            (
                json!({"code": "MCA-100", "note": "hi", "status": "done"}),
                "unknown field",
            ),
            (json!({"note": "no code"}), "missing field `code`"),
        ] {
            let (resp, event) = note(&db, args);
            assert!(!resp.ok, "should have been rejected");
            let e = resp.error.unwrap();
            assert!(e.contains(needle), "expected {needle:?} in {e:?}");
            assert!(event.is_none());
        }
        // Args at all are required, and nothing above wrote a line.
        assert!(!note(&db, Value::Null).0.ok);
        let items = store::list(&db.lock(), "p1").unwrap();
        let trail = events::list_for_item(&db.lock(), &items[0].id).unwrap();
        assert!(
            trail.iter().all(|e| e.kind == EventKind::Proposed),
            "{trail:?}"
        );

        // Exactly at the cap is fine — the refusal is for going over it.
        let (resp, _) = note(
            &db,
            json!({"code": "MCA-100", "note": "y".repeat(MAX_NOTE)}),
        );
        assert!(resp.ok, "{resp:?}");
    }

    /// The note reaches the board through the dispatcher, which is the only path
    /// the PM actually has.
    #[tokio::test]
    async fn note_routes_through_the_dispatcher() {
        let db = test_db("p1");
        let d = dispatcher(&db, "p1");
        assert!(propose(&db, one_item("target")).ok); // MCA-100

        let resp = d
            .dispatch(
                "r1",
                "roadmap_note",
                &json!({"code": "MCA-100", "note": "watch the migration on this one"}),
            )
            .await
            .0;
        assert!(resp.ok, "{resp:?}");
        let items = store::list(&db.lock(), "p1").unwrap();
        let trail = events::list_for_item(&db.lock(), &items[0].id).unwrap();
        assert_eq!(trail[0].kind, EventKind::Note);
        assert_eq!(trail[0].actor, EventActor::Pm);
        // And the listing shows it back, so the PM can see its own note landed.
        let resp = list(&db, Value::Null);
        let rows: Vec<Value> = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
        assert_eq!(rows[0]["last_event"]["kind"], "note");
        assert_eq!(
            rows[0]["last_event"]["detail"],
            "watch the migration on this one"
        );
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
    fn propose_order_parks_the_whole_sequence() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("first")).ok); // MCA-100
        assert!(propose(&db, one_item("second")).ok); // MCA-101
        assert!(propose(&db, one_item("third")).ok); // MCA-102

        let (resp, stored) = propose_order(
            &db,
            json!({"codes": ["MCA-102", "MCA-100", "MCA-101"], "note": "the dep goes first"}),
        );
        assert!(resp.ok, "{resp:?}");
        let out: Value = serde_json::from_str(&resp.stdout.unwrap()).unwrap();
        assert_eq!(
            out["proposed"]["order"],
            json!(["MCA-102", "MCA-100", "MCA-101"])
        );

        // Parked, not applied: the board's order is untouched until the user
        // rules on it.
        let p = stored.unwrap();
        assert_eq!(p.codes, vec!["MCA-102", "MCA-100", "MCA-101"]);
        assert_eq!(p.note.as_deref(), Some("the dep goes first"));
        let rows = store::list(&db.lock(), "p1").unwrap();
        assert_eq!(
            rows.iter().map(|i| i.code.as_str()).collect::<Vec<_>>(),
            vec!["MCA-100", "MCA-101", "MCA-102"]
        );
    }

    #[test]
    fn propose_order_rejects_anything_but_the_exact_orderable_set() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("first")).ok); // MCA-100
        assert!(propose(&db, one_item("second")).ok); // MCA-101
        {
            // An item already being built: its place in the queue is settled.
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
            .unwrap(); // MCA-102
        }

        for (args, needle) in [
            (json!({"codes": []}), "must list every orderable item"),
            // Blank entries are trimmed away, which makes this an empty ask.
            (json!({"codes": ["  "]}), "must list every orderable item"),
            (json!({"codes": ["MCA-100"]}), "MCA-101"),
            (
                json!({"codes": ["MCA-100", "MCA-101", "MCA-999"]}),
                "not an item on this board",
            ),
            (
                json!({"codes": ["MCA-100", "MCA-101", "MCA-102"]}),
                "MCA-102 is active",
            ),
            (
                json!({"codes": ["MCA-100", "MCA-100", "MCA-101"]}),
                "appears twice",
            ),
            // A misspelled field would otherwise be silently dropped.
            (
                json!({"codes": ["MCA-100", "MCA-101"], "notes": "why"}),
                "unknown field",
            ),
        ] {
            let (resp, stored) = propose_order(&db, args);
            assert!(!resp.ok, "should have been rejected");
            let e = resp.error.unwrap();
            assert!(e.contains(needle), "expected {needle:?} in {e:?}");
            assert!(stored.is_none());
        }
        // Args at all are required, and nothing above parked an ask.
        assert!(!propose_order(&db, Value::Null).0.ok);
        assert!(order::get(&db.lock(), "p1").unwrap().is_none());
    }

    #[test]
    fn a_newer_order_ask_replaces_the_pending_one() {
        let db = test_db("p1");
        assert!(propose(&db, one_item("first")).ok); // MCA-100
        assert!(propose(&db, one_item("second")).ok); // MCA-101

        assert!(
            propose_order(&db, json!({"codes": ["MCA-100", "MCA-101"]}))
                .0
                .ok
        );
        let (resp, stored) = propose_order(
            &db,
            json!({"codes": ["MCA-101", "MCA-100"], "note": "changed my mind"}),
        );
        assert!(resp.ok, "{resp:?}");
        // One pending ask per board — the user rules on the current position.
        assert_eq!(order::get(&db.lock(), "p1").unwrap(), stored);
        assert_eq!(stored.unwrap().codes, vec!["MCA-101", "MCA-100"]);
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
