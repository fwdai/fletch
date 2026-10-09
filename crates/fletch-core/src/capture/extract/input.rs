//! What one extraction looks at: the new turns as plain text (the user's
//! message and the agent's final message of each), the workspace's plan, the
//! entity index and the current heads. Assembled from the workspace's own
//! turn rows and the verbatim session records; the records are in each
//! provider's own shape, so the final-message read is a tolerant walk over
//! the handful of shapes the providers write rather than a full adapter.

use serde_json::Value;

use crate::context::model::*;
use crate::context::{compile, render};
use crate::workspace::{SessionRecord, UserTurn};

/// Budget for the turns' text; over it, the newest turns wait for the next
/// run.
pub const MAX_TURNS_CHARS: usize = 30_000;
/// Budget for the entity index.
pub const INDEX_CHARS: usize = 2_000;
/// Budget for the heads; over it, only the heads about entities the turns
/// mention are kept.
pub const MAX_HEADS_CHARS: usize = 12_000;

/// One turn as the model sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnText {
    pub turn_id: String,
    pub user: String,
    /// The agent's final message of the turn; `None` when none was recorded
    /// (the turn is still pending, or the provider keeps no transcript).
    pub assistant: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractInput {
    /// The workspace task or brief, for deviation detection.
    pub plan: Option<String>,
    /// Oldest first, newest last.
    pub turns: Vec<TurnText>,
    pub index: String,
    pub heads: String,
}

impl ExtractInput {
    /// `turns` capped to [`MAX_TURNS_CHARS`]; the index and heads rendered
    /// from `graph`, the heads cut to what the turns mention when over budget.
    pub fn new(plan: Option<String>, turns: Vec<TurnText>, graph: &Graph) -> Self {
        let turns = cap_turns(turns, MAX_TURNS_CHARS);
        let mut input = Self {
            plan: plan.filter(|p| !p.trim().is_empty()),
            turns,
            index: render::render_index(graph, INDEX_CHARS),
            heads: String::new(),
        };
        input.heads = heads_text(graph, &input.conversation());
        input
    }

    /// Characters the user typed, the cost guard's measure.
    pub fn user_chars(&self) -> usize {
        self.turns.iter().map(|t| t.user.chars().count()).sum()
    }

    pub fn last_turn_id(&self) -> Option<&str> {
        self.turns.last().map(|t| t.turn_id.as_str())
    }

    /// The turns as the model reads them.
    pub fn conversation(&self) -> String {
        let mut text = String::new();
        for turn in &self.turns {
            text.push_str(&format!(
                "User (turn {}):\n{}\n\n",
                turn.turn_id,
                turn.user.trim()
            ));
            if let Some(reply) = &turn.assistant {
                text.push_str(&format!("Assistant:\n{}\n\n", reply.trim()));
            }
        }
        text
    }
}

/// The workspace's own turns after `after_turn_id` (all of them when it is
/// `None` or no longer among them, as after a rewind), each paired with the
/// agent's final message in the records between its prompt and the next
/// turn's.
pub fn turns_since(
    turns: &[UserTurn],
    records: &[SessionRecord],
    after_turn_id: Option<&str>,
) -> Vec<TurnText> {
    let own: Vec<&UserTurn> = turns.iter().filter(|t| !t.inherited).collect();
    let start = after_turn_id
        .and_then(|id| own.iter().position(|t| t.turn_id == id))
        .map_or(0, |i| i + 1);
    let seq_of = |turn: &UserTurn| {
        let native = turn.native_id.as_deref()?;
        records
            .iter()
            .find(|r| r.native_id == native)
            .map(|r| r.seq)
    };
    let seqs: Vec<Option<i64>> = own.iter().map(|t| seq_of(t)).collect();

    own[start..]
        .iter()
        .enumerate()
        .map(|(offset, turn)| {
            let i = start + offset;
            let assistant = seqs[i].and_then(|from| {
                let to = seqs[i + 1..]
                    .iter()
                    .flatten()
                    .copied()
                    .next()
                    .unwrap_or(i64::MAX);
                records
                    .iter()
                    .rev()
                    .filter(|r| r.seq > from && r.seq < to)
                    .find_map(|r| assistant_text(&r.body))
            });
            TurnText {
                turn_id: turn.turn_id.clone(),
                user: turn.text.clone(),
                assistant,
            }
        })
        .collect()
}

/// The oldest turns whose text fits `max`; a first turn that alone is over
/// is kept, cut from its end. The newest go, not the oldest: the run's
/// watermark is its last turn, so what is dropped here is read next run.
fn cap_turns(mut turns: Vec<TurnText>, max: usize) -> Vec<TurnText> {
    let size = |t: &TurnText| t.user.len() + t.assistant.as_ref().map_or(0, String::len);
    let mut total = 0;
    let keep = turns
        .iter()
        .take_while(|t| {
            total += size(t);
            total <= max
        })
        .count();
    turns.truncate(keep.max(1));
    if let Some(only) = turns.first_mut().filter(|t| size(t) > max) {
        if let Some(reply) = &mut only.assistant {
            truncate(reply, max.saturating_sub(only.user.len()));
        }
        truncate(&mut only.user, max);
    }
    turns
}

fn truncate(text: &mut String, max: usize) {
    if text.len() <= max {
        return;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

/// `slug: [kind/domain/stance] statement (id)` per current head: every one
/// when that fits [`MAX_HEADS_CHARS`], else only those about entities whose
/// slug, alias or name appears in `text`.
fn heads_text(graph: &Graph, text: &str) -> String {
    let all: Vec<String> = graph
        .assertions
        .iter()
        .filter(|a| compile::is_current(graph, a))
        .map(|a| head_line(graph, a))
        .collect();
    let full = all.join("\n");
    if full.len() <= MAX_HEADS_CHARS {
        return full;
    }

    let haystack = text.to_lowercase();
    let mentioned = |e: &Entity| {
        std::iter::once(&e.slug)
            .chain(&e.aliases)
            .chain(std::iter::once(&e.name))
            .any(|needle| !needle.is_empty() && haystack.contains(&needle.to_lowercase()))
    };
    let mut seen = std::collections::HashSet::new();
    let mut lines = Vec::new();
    for entity in graph
        .entities
        .iter()
        .filter(|e| e.status == EntityStatus::Active && mentioned(e))
    {
        for head in compile::heads_about(graph, &entity.id) {
            if seen.insert(head.id.clone()) {
                lines.push(head_line(graph, head));
            }
        }
    }
    let mut out = String::new();
    for line in lines {
        if out.len() + line.len() + 1 > MAX_HEADS_CHARS {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn head_line(graph: &Graph, a: &Assertion) -> String {
    let slug = a
        .about
        .first()
        .and_then(|id| graph.entity(id))
        .map_or("?", |e| e.slug.as_str());
    format!(
        "{slug}: [{}/{}/{}] {} ({})",
        tag(&a.kind),
        tag(&a.domain),
        tag(&a.stance),
        a.statement,
        a.id
    )
}

fn tag<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The assistant's prose in one verbatim record, for the shapes the providers
/// write: claude and cursor (`type`/`role` `assistant`, `message.content` as
/// text blocks or a string), pi (`message.role`), and codex (`agent_message`
/// items). `None` for anything else, including sub-agent (sidechain) lines.
pub fn assistant_text(body: &Value) -> Option<String> {
    if body.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let is_assistant = |v: Option<&Value>| v.and_then(Value::as_str) == Some("assistant");
    let message = body.get("message");
    if is_assistant(body.get("type"))
        || is_assistant(body.get("role"))
        || is_assistant(message.and_then(|m| m.get("role")))
    {
        let content = message
            .and_then(|m| m.get("content"))
            .or_else(|| body.get("content"))?;
        return text_of(content);
    }
    let payload = body.get("payload")?;
    let item = payload.get("item").unwrap_or(payload);
    if item.get("type").and_then(Value::as_str) == Some("agent_message") {
        return item
            .get("text")
            .or_else(|| item.get("message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
    }
    None
}

/// A content string, or the text blocks of a content array joined.
fn text_of(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}
