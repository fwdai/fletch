//! The labels every Fletch container carries so a later sweep can attribute it.
//! Stamped at launch by [`run_args`](super::run_args), consumed by the runtimes'
//! sweeps (see `sandbox::docker::cleanup`).

/// Label carrying the owning Fletch instance's pid.
pub(crate) const HOST_PID_LABEL: &str = "fletch.host-pid";

/// Label carrying the agent id a container runs — the only stable handle for
/// per-agent teardown, since container *names* carry a random nonce
/// ([`util::container_name`](super::util::container_name)) no other process can
/// reconstruct.
pub(crate) const AGENT_ID_LABEL: &str = "fletch.agent-id";

/// `fletch.host-pid=<our pid>` — the `--label` value stamped on a container run.
pub(crate) fn host_pid_label() -> String {
    format!("{HOST_PID_LABEL}={}", std::process::id())
}

/// `fletch.agent-id=<agent_id>` — sibling of [`host_pid_label`].
pub(crate) fn agent_id_label(agent_id: &str) -> String {
    format!("{AGENT_ID_LABEL}={agent_id}")
}

/// The `--filter` argument selecting one agent's containers, built from
/// [`agent_id_label`] so the query can't drift from what the launch stamped.
pub(crate) fn agent_id_filter(agent_id: &str) -> String {
    format!("label={}", agent_id_label(agent_id))
}

/// `<runtime> inspect` line format pairing each container id with its owning
/// pid. `index` yields an empty string for a missing label, which parses to "no
/// pid" and is skipped — the under-reclaim bias.
pub(crate) const INSPECT_FORMAT: &str = r#"{{.Id}} {{index .Config.Labels "fletch.host-pid"}}"#;

/// Parse [`INSPECT_FORMAT`] output and keep the ids whose owning pid is
/// *provably* dead — an unattributable container is left alone.
pub(crate) fn orphaned_ids(inspect_stdout: &str, alive: impl Fn(i32) -> bool) -> Vec<String> {
    inspect_stdout
        .lines()
        .filter_map(parse_inspect_line)
        .filter(|(_, pid)| pid.is_some_and(|p| !alive(p)))
        .map(|(id, _)| id)
        .collect()
}

/// One [`INSPECT_FORMAT`] line → `(container_id, owning_pid)`; the pid is
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

    /// The template spells the label as a literal (Go templates can't be
    /// `const`-formatted), so a rename would silently empty every pid.
    #[test]
    fn inspect_format_reads_the_host_pid_label() {
        assert!(INSPECT_FORMAT.contains(HOST_PID_LABEL), "{INSPECT_FORMAT}");
    }
}
