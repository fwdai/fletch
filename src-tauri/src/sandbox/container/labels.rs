//! The labels every Fletch container carries, so a later sweep can attribute
//! it: `fletch.host-pid=<pid>` (which app instance owns it) and
//! `fletch.agent-id=<id>` (which agent it runs). Stamped at launch by
//! [`run_args`](super::run_args) and consumed by the runtime's sweeps (see
//! `sandbox::docker::cleanup`).

/// Label carrying the owning Fletch instance's pid.
pub(crate) const HOST_PID_LABEL: &str = "fletch.host-pid";

/// Label carrying the agent id a container runs. The startup orphan sweep
/// keys on [`HOST_PID_LABEL`] alone (it asks "whose instance is dead?", not
/// "which agent?"); this label is the handle for the other question —
/// `cleanup::remove_agent_containers` uses it to tear down one named agent's
/// containers on archive/discard. It is also the only stable handle there is:
/// container *names* carry a random nonce (`engine::util::container_name`),
/// so nothing outside the launching process can reconstruct them.
pub(crate) const AGENT_ID_LABEL: &str = "fletch.agent-id";

/// `fletch.host-pid=<our pid>` — the `--label` value stamped on a container run.
pub(crate) fn host_pid_label() -> String {
    format!("{HOST_PID_LABEL}={}", std::process::id())
}

/// `fletch.agent-id=<agent_id>` — sibling of [`host_pid_label`].
pub(crate) fn agent_id_label(agent_id: &str) -> String {
    format!("{AGENT_ID_LABEL}={agent_id}")
}

/// The `--filter` argument selecting one agent's containers. Built from
/// [`agent_id_label`] so the query can never drift from what the launch
/// stamped.
pub(crate) fn agent_id_filter(agent_id: &str) -> String {
    format!("label={}", agent_id_label(agent_id))
}

/// `<runtime> inspect` line format pairing each container id with its owning
/// pid. `index` (rather than a direct field access) yields an empty string
/// when the label is somehow absent, which parses to "no pid" and is skipped
/// — under-reclaiming, never removing something we can't attribute. Lives here
/// so the template and [`HOST_PID_LABEL`] can't drift (guarded by a test).
pub(crate) const INSPECT_FORMAT: &str = r#"{{.Id}} {{index .Config.Labels "fletch.host-pid"}}"#;

/// Parse [`INSPECT_FORMAT`] output and keep the ids whose owning pid is
/// provably dead. A missing or unparsable pid means we can't attribute the
/// container, so it is left alone (same under-reclaim bias as
/// `cleanup_nested_state_roots_in`). Pure — the liveness probe is injected
/// for unit tests.
pub(crate) fn orphaned_ids(inspect_stdout: &str, alive: impl Fn(i32) -> bool) -> Vec<String> {
    inspect_stdout
        .lines()
        .filter_map(parse_inspect_line)
        .filter(|(_, pid)| pid.is_some_and(|p| !alive(p)))
        .map(|(id, _)| id)
        .collect()
}

/// One [`INSPECT_FORMAT`] line → `(container_id, owning_pid)`. The pid is
/// `None` when the label was empty or not a number.
pub(crate) fn parse_inspect_line(line: &str) -> Option<(String, Option<i32>)> {
    let mut parts = line.split_whitespace();
    let id = parts.next()?;
    let pid = parts.next().and_then(|p| p.parse::<i32>().ok());
    Some((id.to_string(), pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inspect template reads the same label the launch stamps. Spelled as
    /// a literal (Go templates can't be `const`-formatted), so a rename of
    /// [`HOST_PID_LABEL`] would otherwise silently return empty pids for every
    /// container and make the orphan sweep reclaim nothing.
    #[test]
    fn inspect_format_reads_the_host_pid_label() {
        assert!(INSPECT_FORMAT.contains(HOST_PID_LABEL), "{INSPECT_FORMAT}");
    }
}
