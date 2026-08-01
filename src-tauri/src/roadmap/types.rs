//! The `roadmap_items` domain types: one row, its four string-backed enums, and
//! the create/patch payloads the commands accept.
//!
//! The enums use the shared [`crate::db_enum`] macro, so the on-disk spelling
//! and the on-wire spelling are the same string by construction — a `horizon`
//! the frontend sends is literally the value stored in the column.
//!
//! The `*_json` TEXT columns (`accept_json`, `deps_json`) are marshalled here
//! and never leak: [`RoadmapItem`] carries real `Vec<String>`s, so the frontend
//! receives JSON arrays rather than strings-of-JSON.

use rusqlite::types::Type;
use rusqlite::Row;
use serde::{Deserialize, Serialize};

crate::db_enum! {
    /// Where an item sits on the board. `now` is being built, `next` is queued
    /// up, `later` is the backlog. Shipped items leave the board entirely.
    Horizon {
        Now   => "now",
        Next  => "next",
        Later => "later",
    }
}

crate::db_enum! {
    /// Item lifecycle: `proposed → open → queued → active → in_review → done`.
    /// `proposed` is a PM suggestion the user hasn't accepted (a ghost row);
    /// `done` items leave the board and become the header's "shipped" count.
    ItemStatus {
        Proposed => "proposed",
        Open     => "open",
        Queued   => "queued",
        Active   => "active",
        InReview => "in_review",
        Done     => "done",
    }
}

crate::db_enum! {
    /// Rough effort tier. Nullable on the row — an unshaped idea has no size.
    ItemSize {
        Xs => "XS",
        S  => "S",
        M  => "M",
        L  => "L",
    }
}

crate::db_enum! {
    /// Where the item came from. `user` is a hand-written row, `pm` came out of
    /// the roadmap conversation, the rest are imports.
    ItemSource {
        User   => "user",
        Pm     => "pm",
        Linear => "linear",
        Github => "github",
    }
}

/// One `roadmap_items` row as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadmapItem {
    pub id: String,
    pub project_id: String,
    /// Short human id ("FLT-142"), unique per project and never reallocated.
    pub code: String,
    /// Reserved for sub-items; always `None` today (no UI writes it).
    pub parent_id: Option<String>,
    pub title: String,
    /// The one line that justifies the item's place on the board.
    pub why: String,
    pub horizon: Horizon,
    pub status: ItemStatus,
    pub size: Option<ItemSize>,
    /// Product-map domain this belongs to.
    pub area: Option<String>,
    pub source: ItemSource,
    /// Grouping label when several items were shaped together.
    pub epic: Option<String>,
    /// Acceptance criteria, rendered as a checklist. Empty, never null.
    pub accept: Vec<String>,
    /// Codes this item must land after. Empty, never null.
    pub deps: Vec<String>,
    pub agent_id: Option<String>,
    pub workflow_def_id: Option<String>,
    pub run_id: Option<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// The columns every read selects, in one place so the row decoder and the
/// queries can't disagree about what is available.
pub(crate) const COLUMNS: &str = "id, project_id, code, parent_id, title, why, horizon, status, \
     size, area, source, epic, accept_json, deps_json, agent_id, workflow_def_id, run_id, \
     pr_url, pr_number, created_at, updated_at";

impl RoadmapItem {
    pub fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get("id")?,
            project_id: r.get("project_id")?,
            code: r.get("code")?,
            parent_id: r.get("parent_id")?,
            title: r.get("title")?,
            why: r.get("why")?,
            horizon: enum_col(r, "horizon", Horizon::from_db)?,
            status: enum_col(r, "status", ItemStatus::from_db)?,
            size: opt_enum_col(r, "size", ItemSize::from_db)?,
            area: r.get("area")?,
            source: enum_col(r, "source", ItemSource::from_db)?,
            epic: r.get("epic")?,
            accept: strings_col(r, "accept_json")?,
            deps: strings_col(r, "deps_json")?,
            agent_id: r.get("agent_id")?,
            workflow_def_id: r.get("workflow_def_id")?,
            run_id: r.get("run_id")?,
            pr_url: r.get("pr_url")?,
            pr_number: r.get("pr_number")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
        })
    }
}

/// A new item. Everything but `title` has a defined default, so the smallest
/// useful call is a title and nothing else — the board's quick-add path.
/// `code` is never accepted from the caller: it is allocated by
/// [`crate::roadmap::store::create`] under the connection lock.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewItem {
    pub title: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub horizon: Option<Horizon>,
    /// Defaults to `open`. `proposed` is how a PM suggestion is persisted
    /// before the user accepts it.
    #[serde(default)]
    pub status: Option<ItemStatus>,
    #[serde(default)]
    pub size: Option<ItemSize>,
    #[serde(default)]
    pub area: Option<String>,
    /// Defaults to `user` — a row typed on the board by hand.
    #[serde(default)]
    pub source: Option<ItemSource>,
    #[serde(default)]
    pub epic: Option<String>,
    #[serde(default)]
    pub accept: Vec<String>,
    #[serde(default)]
    pub deps: Vec<String>,
    /// Workflow this item is dispatched under when it's queued. `None` means
    /// "whatever the project's default is at dispatch time" — accepted here so
    /// the item form can create and assign in one round-trip.
    #[serde(default)]
    pub workflow_def_id: Option<String>,
}

/// A partial update. An absent field is left alone; an explicit `null` on a
/// nullable column clears it (`Option<Option<T>>` with [`double_option`] —
/// absent is `None`, `null` is `Some(None)`), which is how "unset the size" is
/// expressed without a second command.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ItemPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub why: Option<String>,
    #[serde(default)]
    pub horizon: Option<Horizon>,
    #[serde(default)]
    pub status: Option<ItemStatus>,
    #[serde(default)]
    pub source: Option<ItemSource>,
    #[serde(default)]
    pub accept: Option<Vec<String>>,
    #[serde(default)]
    pub deps: Option<Vec<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub size: Option<Option<ItemSize>>,
    #[serde(default, deserialize_with = "double_option")]
    pub area: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub epic: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub agent_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub workflow_def_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub run_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub pr_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub pr_number: Option<Option<i64>>,
}

/// Keep a double-`Option` field's `null` distinct from its absence. Serde's
/// stock `Option` deserializer folds JSON `null` into the *outer* `None`, which
/// would make "clear this column" unreachable from the frontend — the patch
/// would read as "leave it alone" and the edit dialog's clears would silently
/// revert. This runs only when the key is present, so `null` lands as
/// `Some(None)`; `#[serde(default)]` still covers the absent case.
fn double_option<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

// ───────────────────────────── row helpers ──────────────────────────────

fn conversion_err(col: &str, detail: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, Type::Text, format!("{col}: {detail}").into())
}

/// Parse a nullable JSON-array TEXT column into a `Vec<String>`. NULL and a
/// stored `[]` both read as empty, so callers never branch on which one a
/// writer chose.
fn strings_col(r: &Row, col: &str) -> rusqlite::Result<Vec<String>> {
    let raw: Option<String> = r.get(col)?;
    match raw.as_deref() {
        None | Some("") => Ok(Vec::new()),
        Some(s) => serde_json::from_str(s).map_err(|e| conversion_err(col, e.to_string())),
    }
}

/// Parse a required enum TEXT column via its `from_db`.
fn enum_col<T>(r: &Row, col: &str, parse: fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    let raw: String = r.get(col)?;
    parse(&raw).ok_or_else(|| conversion_err(col, format!("unexpected value {raw:?}")))
}

/// Parse a nullable enum TEXT column via its `from_db`.
fn opt_enum_col<T>(
    r: &Row,
    col: &str,
    parse: fn(&str) -> Option<T>,
) -> rusqlite::Result<Option<T>> {
    let raw: Option<String> = r.get(col)?;
    match raw {
        None => Ok(None),
        Some(s) => parse(&s)
            .map(Some)
            .ok_or_else(|| conversion_err(col, format!("unexpected value {s:?}"))),
    }
}

/// Serialize a string list for its `*_json` column. An empty list stores as
/// NULL rather than `"[]"` — the column's "nothing here" value is one thing.
pub(crate) fn strings_to_col(v: &[String]) -> Option<String> {
    if v.is_empty() {
        None
    } else {
        serde_json::to_string(v).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the wire semantics of the patch: the store tests build `ItemPatch`
    /// in Rust and never touch serde, but the frontend's patches arrive as
    /// JSON through the command layer, where `null` and "absent" are different
    /// bytes that must stay different values.
    #[test]
    fn patch_null_clears_value_sets_absent_keeps() {
        let p: ItemPatch = serde_json::from_str(r#"{"size": null, "area": "runtime"}"#).unwrap();
        assert_eq!(p.size, Some(None), "an explicit null means 'clear'");
        assert_eq!(p.area, Some(Some("runtime".into())), "a value means 'set'");
        assert_eq!(p.epic, None, "an absent key means 'leave alone'");
        assert_eq!(p.workflow_def_id, None);
        assert_eq!(p.title, None);
    }
}
