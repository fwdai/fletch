//! `tour_set`: the agent's walkthrough of its changes. Validated here and
//! relayed as [`EVENT_TOUR_SET`]; the supervisor's watcher stores it, as it
//! does a title. No git runs, and it is neither a publish nor a mutating op.

use serde_json::Value;

use super::{GitDispatcher, EVENT_TOUR_SET};
use crate::rpc::{Response, RpcEvent};
use crate::workspace::{TourAnchor, TourStep};

const MAX_STEPS: usize = 40;
const MAX_NOTE_CHARS: usize = 600;
const MAX_STEP_TITLE_CHARS: usize = 80;
const MAX_ANCHORS: usize = 12;

impl GitDispatcher {
    pub(super) fn tour_set(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        match tour_steps(args) {
            Ok(steps) => (
                Response::ok(id, 0, String::new(), String::new()),
                vec![RpcEvent::named(
                    EVENT_TOUR_SET,
                    serde_json::json!({ "steps": steps }),
                )],
            ),
            Err(why) => (Response::err(id, format!("tour_set: {why}")), Vec::new()),
        }
    }
}

/// `args.steps` with each step's text whitespace-collapsed, or the first thing
/// wrong with them, named by position (`steps[2].anchors[0].path`).
fn tour_steps(args: &Value) -> Result<Vec<TourStep>, String> {
    let Some(steps) = args.get("steps").and_then(Value::as_array) else {
        return Err("`steps` must be an array of steps".into());
    };
    if steps.is_empty() || steps.len() > MAX_STEPS {
        return Err(format!(
            "`steps` must hold 1 to {MAX_STEPS} steps, got {}",
            steps.len()
        ));
    }
    steps
        .iter()
        .enumerate()
        .map(|(i, step)| tour_step(&format!("steps[{i}]"), step))
        .collect()
}

fn tour_step(at: &str, step: &Value) -> Result<TourStep, String> {
    if !step.is_object() {
        return Err(format!("{at} must be an object"));
    }
    let note = text(at, step, "note")?.unwrap_or_default();
    if note.is_empty() {
        return Err(format!("{at}.note is required"));
    }
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(format!(
            "{at}.note exceeds {MAX_NOTE_CHARS} characters; keep it to one or two sentences"
        ));
    }
    let title = text(at, step, "title")?.filter(|t| !t.is_empty());
    if title
        .as_ref()
        .is_some_and(|t| t.chars().count() > MAX_STEP_TITLE_CHARS)
    {
        return Err(format!(
            "{at}.title exceeds {MAX_STEP_TITLE_CHARS} characters"
        ));
    }
    let Some(anchors) = step.get("anchors").and_then(Value::as_array) else {
        return Err(format!(
            "{at}.anchors must be an array (empty for a step with no code)"
        ));
    };
    if anchors.len() > MAX_ANCHORS {
        return Err(format!(
            "{at}.anchors holds {} anchors; at most {MAX_ANCHORS}",
            anchors.len()
        ));
    }
    let anchors = anchors
        .iter()
        .enumerate()
        .map(|(j, anchor)| tour_anchor(&format!("{at}.anchors[{j}]"), anchor))
        .collect::<Result<_, _>>()?;
    Ok(TourStep {
        title,
        note,
        anchors,
    })
}

/// `step[key]` with its whitespace collapsed; `None` when absent or null.
fn text(at: &str, step: &Value, key: &str) -> Result<Option<String>, String> {
    match step.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.split_whitespace().collect::<Vec<_>>().join(" "))),
        Some(_) => Err(format!("{at}.{key} must be a string")),
    }
}

fn tour_anchor(at: &str, anchor: &Value) -> Result<TourAnchor, String> {
    let path = anchor
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if path.is_empty() {
        return Err(format!("{at}.path is required"));
    }
    if path.starts_with('/') || path.split('/').any(|segment| segment == "..") {
        return Err(format!(
            "{at}.path must be relative, with no `..` segment: {path:?}"
        ));
    }
    let line = |key: &str| {
        anchor
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|&n| n >= 1)
            .ok_or_else(|| format!("{at}.{key} must be a line number, counting from 1"))
    };
    let (start, end) = (line("start")?, line("end")?);
    if end < start {
        return Err(format!("{at}: end {end} is before start {start}"));
    }
    Ok(TourAnchor {
        path: path.to_string(),
        start,
        end,
    })
}
