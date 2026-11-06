//! `roadmap_triage`: the PM classifies a captured row. Every outcome is a
//! proposal — the row itself as a ghost (`ask`), or ghosts linked to it
//! (`idea`, `brainstorm`) — so nothing reaches the board unruled, and nothing
//! here can queue, accept or release anything.

use rusqlite::Connection;
use serde_json::{json, Map, Value};

use crate::roadmap::deps;
use crate::roadmap::events::{self, EventActor, EventKind};
use crate::roadmap::store;
use crate::roadmap::types::{CaptureKind, ItemPatch, ItemStatus, NewItem, RoadmapItem, Triage};
use crate::rpc::Response;

use super::args::{clean_list, one_of, parse_required, wrote, ProposedItem, TriageArgs};
use super::duplicates::duplicate_warnings;
use super::intake::{create_proposed, created_codes, validate, Proposed};

const MAX_QUESTIONS: usize = 3;

const MAX_BRIEF: usize = 1000;

enum Outcome {
    Ask(ItemPatch),
    Idea {
        triage: Triage,
        candidates: Vec<NewItem>,
    },
    Brainstorm {
        candidates: Vec<NewItem>,
    },
}

pub(super) fn triage_op(
    conn: &Connection,
    project_id: &str,
    id: &str,
    args: &Value,
) -> (Response, Option<Proposed>) {
    wrote(
        id,
        "roadmap_triage",
        "triaged",
        triage(conn, project_id, args),
    )
}

fn captured<'a>(board: &'a [RoadmapItem], code: &str) -> Result<&'a RoadmapItem, String> {
    let item = board.iter().find(|i| i.code == code).ok_or_else(|| {
        format!("no item {code:?} on this board — `roadmap_list` shows what exists")
    })?;
    if item.status != ItemStatus::Captured {
        return Err(format!(
            "{} is {} — only a captured row is triaged; reshape a ticket with \
             roadmap_propose_update",
            item.code,
            item.status.as_str()
        ));
    }
    Ok(item)
}

fn refuse_extra(kind: CaptureKind, takes: &str, extra: &[(&str, bool)]) -> Result<(), String> {
    match extra.iter().find(|(_, present)| *present) {
        Some((field, _)) => Err(format!(
            "`{field}` doesn't belong to {} — it takes {takes}",
            kind.as_str()
        )),
        None => Ok(()),
    }
}

/// Validated candidates, linked to the captured row. A candidate may not wait
/// on the captured row itself: it is a note, not a ticket, and never lands.
fn candidates(
    items: &[ProposedItem],
    source: &RoadmapItem,
    board: &[RoadmapItem],
) -> Result<Vec<NewItem>, String> {
    let mut news = validate(items, board).map_err(|e| e.replace("`items`", "`candidates`"))?;
    for (n, new) in news.iter_mut().enumerate() {
        if new.deps.contains(&source.code) {
            return Err(format!(
                "candidate {} ({:?}): {} is the captured note, not a ticket — nothing \
                 can wait on it",
                n + 1,
                new.title,
                source.code
            ));
        }
        new.from_item = Some(source.id.clone());
    }
    Ok(news)
}

fn shape(args: TriageArgs, source: &RoadmapItem, board: &[RoadmapItem]) -> Result<Outcome, String> {
    let kind = CaptureKind::from_db(args.kind.trim()).ok_or_else(|| {
        format!(
            "unknown kind {:?} — expected {}",
            args.kind,
            one_of(&["ask", "idea", "brainstorm"])
        )
    })?;
    match kind {
        CaptureKind::Ask => {
            refuse_extra(
                kind,
                "`ticket`",
                &[
                    ("brief", args.brief.is_some()),
                    ("questions", args.questions.is_some()),
                    ("candidates", args.candidates.is_some()),
                ],
            )?;
            let ticket = args
                .ticket
                .ok_or("`ticket` is required for an ask — the row becomes that ticket")?;
            let new = validate(std::slice::from_ref(&ticket), board)
                .map_err(|e| e.replace("item 1", "`ticket`"))?
                .remove(0);
            deps::validate_edit(
                &deps::graph_of(board),
                &deps::rejected_of(board),
                &source.code,
                &new.deps,
            )?;
            Ok(Outcome::Ask(ItemPatch {
                title: Some(new.title),
                why: Some(new.why),
                horizon: new.horizon,
                status: Some(ItemStatus::Proposed),
                area: Some(new.area),
                accept: Some(new.accept),
                deps: Some(new.deps),
                ..Default::default()
            }))
        }
        CaptureKind::Idea => {
            refuse_extra(
                kind,
                "`brief`, `questions` and optional `candidates`",
                &[("ticket", args.ticket.is_some())],
            )?;
            let brief = args.brief.as_deref().map(str::trim).unwrap_or_default();
            if brief.is_empty() {
                return Err("`brief` is required for an idea — what is worth finding out".into());
            }
            let length = brief.chars().count();
            if length > MAX_BRIEF {
                return Err(format!(
                    "`brief` is {length} characters — keep it under {MAX_BRIEF}"
                ));
            }
            let questions = clean_list(args.questions.as_deref().unwrap_or_default());
            if questions.len() > MAX_QUESTIONS {
                return Err(format!(
                    "{} questions is too many — ask the {MAX_QUESTIONS} that decide it",
                    questions.len()
                ));
            }
            let candidates = match args.candidates.as_deref() {
                None | Some([]) => Vec::new(),
                Some(items) => candidates(items, source, board)?,
            };
            Ok(Outcome::Idea {
                triage: Triage {
                    brief: Some(brief.to_string()),
                    questions,
                    mined: false,
                },
                candidates,
            })
        }
        CaptureKind::Brainstorm => {
            refuse_extra(
                kind,
                "`candidates`",
                &[
                    ("ticket", args.ticket.is_some()),
                    ("brief", args.brief.is_some()),
                    ("questions", args.questions.is_some()),
                ],
            )?;
            let items = args.candidates.unwrap_or_default();
            if items.is_empty() {
                return Err(
                    "`candidates` must name at least one ticket worth taking from it".into(),
                );
            }
            Ok(Outcome::Brainstorm {
                candidates: candidates(&items, source, board)?,
            })
        }
    }
}

fn codes(items: &[RoadmapItem]) -> String {
    items
        .iter()
        .map(|i| i.code.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn triage(conn: &Connection, project_id: &str, args: &Value) -> Result<(Value, Proposed), String> {
    let args: TriageArgs = parse_required(args)?;
    let board = store::list(conn, project_id).map_err(|e| e.to_string())?;
    let source = captured(&board, args.code.trim())?.clone();
    let outcome = shape(args, &source, &board)?;
    let warnings = match &outcome {
        Outcome::Ask(_) => Vec::new(),
        Outcome::Idea { candidates, .. } | Outcome::Brainstorm { candidates } => {
            duplicate_warnings(candidates, &board)
        }
    };

    let (kind, (items, recorded)) = (|| -> Result<(CaptureKind, Proposed), String> {
        let db = |e: rusqlite::Error| e.to_string();
        let tx = conn.unchecked_transaction().map_err(db)?;
        let note = |detail: &str| {
            events::record(
                &tx,
                &source.id,
                project_id,
                EventActor::Pm,
                EventKind::Note,
                Some(detail),
            )
        };
        let from = format!("from {}", source.code);
        let out = match outcome {
            Outcome::Ask(patch) => {
                store::update_where_status(&tx, &source.id, ItemStatus::Captured, &patch)
                    .map_err(db)?
                    .ok_or_else(|| format!("{} was triaged meanwhile", source.code))?;
                let row = store::set_triage(&tx, &source.id, CaptureKind::Ask, None)
                    .map_err(db)?
                    .ok_or_else(|| store::missing(&source.id))?;
                let event = events::record(
                    &tx,
                    &source.id,
                    project_id,
                    EventActor::Pm,
                    EventKind::Proposed,
                    Some("triaged as ask"),
                )
                .map_err(db)?;
                (CaptureKind::Ask, (vec![row], vec![event]))
            }
            Outcome::Idea { triage, candidates } => {
                let (mut items, mut recorded) =
                    create_proposed(&tx, project_id, &candidates, Some(&from)).map_err(db)?;
                let mut detail = format!(
                    "triaged as idea — brief, {} question{}",
                    triage.questions.len(),
                    if triage.questions.len() == 1 { "" } else { "s" }
                );
                if !items.is_empty() {
                    detail.push_str(&format!("; proposed {}", codes(&items)));
                }
                let row = store::set_triage(&tx, &source.id, CaptureKind::Idea, Some(&triage))
                    .map_err(db)?
                    .ok_or_else(|| store::missing(&source.id))?;
                recorded.push(note(&detail).map_err(db)?);
                items.insert(0, row);
                (CaptureKind::Idea, (items, recorded))
            }
            Outcome::Brainstorm { candidates } => {
                let (mut items, mut recorded) =
                    create_proposed(&tx, project_id, &candidates, Some(&from)).map_err(db)?;
                let mined = Triage {
                    mined: true,
                    ..Default::default()
                };
                let row = store::set_triage(&tx, &source.id, CaptureKind::Brainstorm, Some(&mined))
                    .map_err(db)?
                    .ok_or_else(|| store::missing(&source.id))?;
                recorded.push(
                    note(&format!("triaged as brainstorm — mined {}", codes(&items)))
                        .map_err(db)?,
                );
                items.insert(0, row);
                (CaptureKind::Brainstorm, (items, recorded))
            }
        };
        tx.commit().map_err(db)?;
        Ok(out)
    })()?;

    let mut triaged = Map::new();
    triaged.insert("code".into(), json!(source.code));
    triaged.insert("kind".into(), json!(kind.as_str()));
    let mut payload = Map::new();
    payload.insert("triaged".into(), Value::Object(triaged));
    // The captured row leads `items`; anything after it was proposed from it.
    if items.len() > 1 {
        payload.insert("created".into(), created_codes(&items[1..]));
    }
    if !warnings.is_empty() {
        payload.insert("warnings".into(), json!(warnings));
    }
    Ok((Value::Object(payload), (items, recorded)))
}

#[cfg(test)]
#[path = "tests/triage.rs"]
mod tests;
