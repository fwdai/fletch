//! Roadmap item row, enums, and create/patch payloads.
//!
//! Enums use [`crate::db_enum`] so on-disk and on-wire spellings match.
//! `*_json` TEXT columns are marshalled here — [`RoadmapItem`] exposes real vecs.


use rusqlite::types::Type;
use rusqlite::Row;
use serde::{Deserialize, Serialize};

crate::db_enum! {
    Horizon {
        Now   => "now",
        Next  => "next",
        Later => "later",
    }
}

crate::db_enum! {
    ItemStatus {
        Proposed => "proposed",
        Open     => "open",
        Queued   => "queued",
        Active   => "active",
        InReview => "in_review",
        Done     => "done",
        Rejected => "rejected",
    }
}

impl ItemStatus {
    /// Proposed/open/queued only; holds do not seal the shape.
    pub fn is_rulable(self) -> bool {
        matches!(self, Self::Proposed | Self::Open | Self::Queued)
    }
}

crate::db_enum! {
    ItemSource {
        User   => "user",
        Pm     => "pm",
        Linear => "linear",
        Github => "github",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadmapItem {
    pub id: String,
    pub project_id: String,
    pub code: String,
    pub title: String,
    pub why: String,
    pub horizon: Horizon,
    pub status: ItemStatus,
    pub rank: f64,
    pub area: Option<String>,
    pub source: ItemSource,
    pub accept: Vec<String>,
    pub deps: Vec<String>,
    pub agent_id: Option<String>,
    pub workflow_def_id: Option<String>,
    pub run_id: Option<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
    pub hold_reason: Option<String>,
    pub held_by: Option<super::events::EventActor>,
    pub held_at: Option<i64>,
    pub close_reason: Option<String>,
    pub issue_url: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub(crate) const COLUMNS: &str = "id, project_id, code, title, why, horizon, status, \
     rank, area, source, accept_json, deps_json, agent_id, workflow_def_id, run_id, \
     pr_url, pr_number, hold_reason, held_by, held_at, close_reason, issue_url, \
     created_at, updated_at";

impl RoadmapItem {
    pub fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get("id")?,
            project_id: r.get("project_id")?,
            code: r.get("code")?,
            title: r.get("title")?,
            why: r.get("why")?,
            horizon: enum_col(r, "horizon", Horizon::from_db)?,
            status: enum_col(r, "status", ItemStatus::from_db)?,
            rank: r.get("rank")?,
            area: r.get("area")?,
            source: enum_col(r, "source", ItemSource::from_db)?,
            accept: strings_col(r, "accept_json")?,
            deps: strings_col(r, "deps_json")?,
            agent_id: r.get("agent_id")?,
            workflow_def_id: r.get("workflow_def_id")?,
            run_id: r.get("run_id")?,
            pr_url: r.get("pr_url")?,
            pr_number: r.get("pr_number")?,
            hold_reason: r.get("hold_reason")?,
            held_by: opt_enum_col(r, "held_by", super::events::EventActor::from_db)?,
            held_at: r.get("held_at")?,
            close_reason: r.get("close_reason")?,
            issue_url: r.get("issue_url")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
        })
    }

    pub fn is_held(&self) -> bool {
        self.hold_reason.is_some()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewItem {
    pub title: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub horizon: Option<Horizon>,
    #[serde(default)]
    pub status: Option<ItemStatus>,
    #[serde(default)]
    pub area: Option<String>,
    #[serde(default)]
    pub source: Option<ItemSource>,
    #[serde(default)]
    pub accept: Vec<String>,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub workflow_def_id: Option<String>,
    #[serde(default)]
    pub issue_url: Option<String>,
}

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
    pub rank: Option<f64>,
    #[serde(default)]
    pub accept: Option<Vec<String>>,
    #[serde(default)]
    pub deps: Option<Vec<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub area: Option<Option<String>>,
    #[serde(skip)]
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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemUpdate {
    pub applied: bool,
    pub item: RoadmapItem,
}

pub(crate) fn double_option<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

fn conversion_err(col: &str, detail: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, Type::Text, format!("{col}: {detail}").into())
}

fn strings_col(r: &Row, col: &str) -> rusqlite::Result<Vec<String>> {
    let raw: Option<String> = r.get(col)?;
    match raw.as_deref() {
        None | Some("") => Ok(Vec::new()),
        Some(s) => serde_json::from_str(s).map_err(|e| conversion_err(col, e.to_string())),
    }
}

pub(crate) fn enum_col<T>(r: &Row, col: &str, parse: fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    let raw: String = r.get(col)?;
    parse(&raw).ok_or_else(|| conversion_err(col, format!("unexpected value {raw:?}")))
}

pub(crate) fn opt_enum_col<T>(
    r: &Row,
    col: &str,
    parse: fn(&str) -> Option<T>,
) -> rusqlite::Result<Option<T>> {
    let raw: Option<String> = r.get(col)?;
    match raw.as_deref() {
        None => Ok(None),
        Some(s) => parse(s)
            .map(Some)
            .ok_or_else(|| conversion_err(col, format!("unexpected value {s:?}"))),
    }
}

pub(crate) fn strings_to_col(v: &[String]) -> Option<String> {
    if v.is_empty() {
        None
    } else {
        serde_json::to_string(v).ok()
    }
}

#[cfg(test)]
#[path = "tests/types.rs"]
mod tests;
