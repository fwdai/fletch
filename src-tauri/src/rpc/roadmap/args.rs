use serde::Deserialize;
use serde_json::Value;

use crate::roadmap::proposals::ProposalPatch;
use crate::rpc::Response;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListArgs {
    #[serde(default)]
    pub(super) status: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeArgs {
    #[serde(default)]
    pub(super) items: Vec<ProposedItem>,
}

/// Not [`NewItem`]: the agent may not choose `status` or `source`, and an
/// unknown field such as a misspelled `horizen` fails loudly.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposedItem {
    #[serde(default)]
    pub(super) title: String,
    #[serde(default)]
    pub(super) why: String,
    #[serde(default)]
    pub(super) horizon: Option<String>,
    #[serde(default)]
    pub(super) area: Option<String>,
    #[serde(default)]
    pub(super) accept: Vec<String>,
    #[serde(default)]
    pub(super) deps: Vec<String>,
}

/// `deny_unknown_fields` on [`ProposalPatch`] is what refuses `status`/`code`/`source`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeUpdateArgs {
    pub(super) code: String,
    pub(super) patch: ProposalPatch,
    #[serde(default)]
    pub(super) note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeDiscardArgs {
    pub(super) code: String,
    #[serde(default)]
    pub(super) reason: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeOrderArgs {
    #[serde(default)]
    pub(super) codes: Vec<String>,
    #[serde(default)]
    pub(super) note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NoteArgs {
    pub(super) code: String,
    #[serde(default)]
    pub(super) note: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HoldArgs {
    /// An item code, or `"project"` for the whole board.
    pub(super) scope: String,
    #[serde(default)]
    pub(super) reason: String,
}

/// Explicit empty shape, so a guessed filter is refused instead of silently ignored.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BriefArgs {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeBriefArgs {
    #[serde(default)]
    pub(super) content: String,
    #[serde(default)]
    pub(super) note: Option<String>,
}

pub(super) const PROJECT_SCOPE: &str = "project";

/// A missing `args` arrives as JSON null, same as `{}`.
pub(super) fn parse_args<T: Default + serde::de::DeserializeOwned>(
    args: &Value,
) -> Result<T, String> {
    if args.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(args.clone()).map_err(|e| e.to_string())
}

pub(super) fn parse_required<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T, String> {
    if args.is_null() {
        return Err("`args` are required".into());
    }
    serde_json::from_value(args.clone()).map_err(|e| e.to_string())
}

pub(super) fn clean(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(super) fn clean_list(v: &[String]) -> Vec<String> {
    v.iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub(super) fn one_of(values: &[&str]) -> String {
    values.join(" | ")
}

fn refuse(id: &str, op: &str, msg: String) -> Response {
    Response::err(id, format!("{op}: {msg}"))
}

pub(super) fn read(id: &str, op: &str, result: Result<Value, String>) -> Response {
    let stdout =
        result.and_then(|payload| serde_json::to_string(&payload).map_err(|e| e.to_string()));
    match stdout {
        Ok(stdout) => Response::ok(id, 0, stdout, String::new()),
        Err(msg) => refuse(id, op, msg),
    }
}

/// On a serialization failure the stored ask is not announced.
pub(super) fn parked<T>(
    id: &str,
    op: &str,
    result: Result<(Value, T), String>,
) -> (Response, Option<T>) {
    match result {
        Ok((payload, stored)) => match serde_json::to_string(&payload) {
            Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), Some(stored)),
            Err(e) => (refuse(id, op, e.to_string()), None),
        },
        Err(msg) => (refuse(id, op, msg), None),
    }
}

/// The write happened even if the payload won't serialize: refuse with
/// "{did}, but …" and still announce it.
pub(super) fn wrote<T>(
    id: &str,
    op: &str,
    did: &str,
    result: Result<(Value, T), String>,
) -> (Response, Option<T>) {
    match result {
        Ok((payload, stored)) => match serde_json::to_string(&payload) {
            Ok(stdout) => (Response::ok(id, 0, stdout, String::new()), Some(stored)),
            Err(e) => (refuse(id, op, format!("{did}, but {e}")), Some(stored)),
        },
        Err(msg) => (refuse(id, op, msg), None),
    }
}
