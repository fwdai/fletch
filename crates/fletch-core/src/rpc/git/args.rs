use serde_json::Value;

use crate::rpc::Response;

pub(super) fn arg_branch(args: &Value) -> Option<String> {
    arg_branch_named(args, "branch")
}

/// A leading `-` could be read by git as an option. `git::push` uses a
/// fully-qualified refspec, but the same strings reach `pr_create`, the fetch
/// argv and event payloads.
pub(super) fn refuse_option_like(id: &str, op: &str, kind: &str, value: &str) -> Option<Response> {
    value
        .starts_with('-')
        .then(|| Response::err(id, format!("{op}: refusing option-like {kind} {value:?}")))
}

pub(super) fn arg_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

pub(super) fn arg_branch_named(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
