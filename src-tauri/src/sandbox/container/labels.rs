//! Container labels stamped on every Docker/Podman launch so orphan sweeps and
//! per-agent disposal can find them without knowing the runtime-specific
//! container name (names carry a launch-time nonce).

/// Label carrying the owning Fletch instance's pid.
pub const HOST_PID_LABEL: &str = "fletch.host-pid";

/// Label carrying the agent id a container runs. The startup orphan sweep keys
/// on [`HOST_PID_LABEL`] alone (it asks "whose instance is dead?", not "which
/// agent?"); this label is the handle for the other question — removing one
/// named agent's containers on archive/discard. It is also the only stable
/// handle there is: container *names* carry a random nonce, so nothing outside
/// the launching process can reconstruct them.
pub const AGENT_ID_LABEL: &str = "fletch.agent-id";

/// `fletch.host-pid=<our pid>` — the `--label` value stamped on `run`.
pub fn host_pid_label() -> String {
    format!("{HOST_PID_LABEL}={}", std::process::id())
}

/// `fletch.agent-id=<agent_id>` — sibling of [`host_pid_label`].
pub fn agent_id_label(agent_id: &str) -> String {
    format!("{AGENT_ID_LABEL}={agent_id}")
}

/// The `--filter` argument selecting one agent's containers. Built from
/// [`agent_id_label`] so the query can never drift from what `run` stamped.
pub fn agent_id_filter(agent_id: &str) -> String {
    format!("label={}", agent_id_label(agent_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_argv_shapes() {
        assert_eq!(
            host_pid_label(),
            format!("fletch.host-pid={}", std::process::id()),
        );
        assert_eq!(agent_id_label("agent-42"), "fletch.agent-id=agent-42");
        assert_eq!(
            agent_id_filter("agent-42"),
            "label=fletch.agent-id=agent-42",
        );
    }
}
