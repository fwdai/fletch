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
