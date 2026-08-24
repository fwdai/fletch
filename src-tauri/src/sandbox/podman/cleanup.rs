//! The dead-instance orphan sweep and the per-agent container removal, for
//! podman.
//!
//! The same two questions [`docker::cleanup`](crate::sandbox::docker) answers,
//! against the same labels ([`container::labels`](crate::sandbox::container::labels)):
//! "whose owning instance is dead?" (startup) and "is anything still running for
//! *this* agent?" (archive/discard). The label parsing and the under-reclaim
//! bias are shared with docker; only the invocations differ.
//!
//! One invocation shape does differ in kind: docker has a single daemon, while
//! podman has one container store per machine and launches pin themselves to
//! the connection they resolved at the time (see [`super::machine`]). Asking
//! only the current default would strand containers on every other machine, so
//! both sweeps run once per machine connection.
//!
//! No image GC here — Podman ships without one for now, so a superseded agent
//! image stays in the local store until the user reclaims it.

use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::container::labels::{
    agent_id_filter, orphaned_ids, HOST_PID_LABEL, INSPECT_FORMAT,
};

use super::cli;

/// Listing/inspect are metadata-only; generous next to their usual
/// milliseconds, so tripping one means the machine connection is wedged.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// `podman rm -f` also kills a still-running container's process; give the
/// batched removal room without letting a hung machine pin the sweep thread.
const REMOVE_TIMEOUT: Duration = Duration::from_secs(60);

/// Remove every fletch-labeled container whose owning host instance is dead,
/// across every machine — a launch is pinned to the connection it resolved at
/// the time, so yesterday's containers can sit on a machine that is no longer
/// the default. Returns the number removed. Callers gate on the probe and run
/// this off the main thread — see [`super::sweep_orphans_at_startup`].
pub(super) fn sweep_orphans() -> Result<usize> {
    across_machines("podman orphan sweep", sweep_orphans_on)
}

/// One sweep pass against a single connection.
fn sweep_orphans_on(connection: Option<&str>) -> Result<usize> {
    let ids = list_ids(connection, &format!("label={HOST_PID_LABEL}"))?;
    if ids.is_empty() {
        return Ok(0);
    }

    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut inspect_args = vec!["inspect", "-f", INSPECT_FORMAT];
    inspect_args.extend(&id_refs);
    let inspected = cli::run_podman_on(connection, &inspect_args, QUERY_TIMEOUT)?;
    // Don't require a zero exit: inspect exits non-zero if ANY id vanished
    // between ps and inspect (e.g. a `--rm` container finishing), but still
    // prints the rows it found — those are the ones we act on.
    let stdout = String::from_utf8_lossy(&inspected.stdout);
    let orphans = orphaned_ids(&stdout, crate::sandbox::seatbelt::pid_alive);
    if orphans.is_empty() {
        return Ok(0);
    }

    tracing::info!(
        count = orphans.len(),
        "removing podman containers of dead fletch instances"
    );
    remove(
        connection,
        &orphans.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    Ok(orphans.len())
}

/// Remove every container stamped with `fletch.agent-id=<agent_id>`, running or
/// not. Returns the number removed.
///
/// The disposal counterpart to [`sweep_orphans`], and deliberately *without*
/// its pid-liveness check: the caller has decided this specific agent is going
/// away, so every container bearing its id is fair game — including one owned
/// by this very live instance, which is the whole point (the supervisor may no
/// longer hold a kill handle for it). Attribution is still exact: the label is
/// stamped only by our own launches, and it names one agent.
///
/// Matching on the label rather than the container name is required, not a
/// preference — names carry a random nonce
/// ([`container::util::container_name`](crate::sandbox::container::util::container_name)),
/// so only the launching process ever knew them.
pub fn remove_agent_containers(agent_id: &str) -> Result<usize> {
    across_machines("podman disposal removal", |connection| {
        remove_agent_containers_on(connection, agent_id)
    })
}

/// One removal pass against a single connection.
fn remove_agent_containers_on(connection: Option<&str>, agent_id: &str) -> Result<usize> {
    let ids = list_ids(connection, &agent_id_filter(agent_id))?;
    if ids.is_empty() {
        return Ok(0);
    }
    tracing::info!(
        agent_id,
        count = ids.len(),
        "removing podman containers of a disposed agent"
    );
    remove(
        connection,
        &ids.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    Ok(ids.len())
}

/// Run `pass` once per machine connection and total what it reclaimed, or once
/// unpinned when there are no machine connections (native Linux, where the
/// single local endpoint is the whole story).
///
/// Best-effort per connection, in keeping with the under-reclaim bias: a
/// stopped or wedged machine logs and the remaining ones still get swept —
/// stopping at the first failure would leave reclaimable containers on
/// machines that answer fine.
fn across_machines(
    what: &str,
    mut pass: impl FnMut(Option<&str>) -> Result<usize>,
) -> Result<usize> {
    let connections = machine_connections();
    if connections.is_empty() {
        return pass(None);
    }
    let mut total = 0;
    for connection in &connections {
        match pass(Some(connection)) {
            Ok(n) => total += n,
            Err(e) => tracing::warn!(
                target: "fletch::podman",
                connection = %connection,
                error = %e,
                "{what} failed on this connection",
            ),
        }
    }
    Ok(total)
}

/// One connection name per Podman machine, from
/// `podman system connection list --format json`. Anything we can't run or read
/// yields none, which the caller reads as "just use the default".
fn machine_connections() -> Vec<String> {
    let Ok(out) = cli::run_podman(
        &["system", "connection", "list", "--format", "json"],
        QUERY_TIMEOUT,
    ) else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_machine_connections(&String::from_utf8_lossy(&out.stdout))
}

/// The machine connection names in a connection listing: entries not explicitly
/// `IsMachine: false` (older podman omits the field, and a non-machine entry is
/// a socket or a remote host, neither of which is ours to sweep).
///
/// Each machine appears twice, as `<machine>` and `<machine>-root`. Both list
/// containers of the same machine, so the pair collapses to whichever podman
/// listed first — sweeping both would ask the same machine twice.
fn parse_machine_connections(stdout: &str) -> Vec<String> {
    let Ok(connections) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return Vec::new();
    };
    let mut seen: Vec<&str> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for connection in &connections {
        if connection.get("IsMachine").and_then(|m| m.as_bool()) == Some(false) {
            continue;
        }
        let Some(name) = connection
            .get("Name")
            .and_then(|n| n.as_str())
            .map(str::trim)
        else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let machine = name.strip_suffix("-root").unwrap_or(name);
        if seen.contains(&machine) {
            continue;
        }
        seen.push(machine);
        out.push(name.to_string());
    }
    out
}

/// Container ids matching one `--filter` expression, on one connection.
fn list_ids(connection: Option<&str>, filter: &str) -> Result<Vec<String>> {
    let list = cli::run_podman_on(
        connection,
        &["ps", "-aq", "--filter", filter],
        QUERY_TIMEOUT,
    )?;
    if !list.status.success() {
        return Err(Error::Other(format!(
            "podman ps failed: {}",
            String::from_utf8_lossy(&list.stderr).trim(),
        )));
    }
    Ok(String::from_utf8_lossy(&list.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// `podman rm -f` over a batch of ids, on one connection.
fn remove(connection: Option<&str>, ids: &[&str]) -> Result<()> {
    let mut rm_args = vec!["rm", "-f"];
    rm_args.extend(ids);
    let removed = cli::run_podman_on(connection, &rm_args, REMOVE_TIMEOUT)?;
    if !removed.status.success() {
        return Err(Error::Other(format!(
            "podman rm failed: {}",
            String::from_utf8_lossy(&removed.stderr).trim(),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::container::labels::host_pid_label;

    /// The sweep set is one connection per machine: rootful pairs collapse
    /// (both name the same container store), non-machine entries are somebody
    /// else's containers, and an unreadable listing leaves the caller with the
    /// default connection alone.
    #[test]
    fn machine_connections_are_one_per_machine() {
        let listing = r#"[
          { "Name": "podman-machine-default", "IsMachine": true, "Default": true },
          { "Name": "podman-machine-default-root", "IsMachine": true, "Default": false },
          { "Name": "work-vm-root", "IsMachine": true, "Default": false },
          { "Name": "work-vm", "IsMachine": true, "Default": false },
          { "Name": "build-box", "IsMachine": false, "Default": false, "URI": "ssh://core@build.example.com:22/run/podman/podman.sock" },
          { "Name": "local", "IsMachine": false, "Default": false, "URI": "unix:///run/podman/podman.sock" },
          { "Name": "  ", "IsMachine": true },
          { "IsMachine": true },
          { "Name": "legacy-no-flag" }
        ]"#;
        assert_eq!(
            parse_machine_connections(listing),
            ["podman-machine-default", "work-vm-root", "legacy-no-flag"],
        );

        assert!(parse_machine_connections("[]").is_empty());
        assert!(parse_machine_connections("").is_empty());
        assert!(parse_machine_connections("Error: no such thing").is_empty());
        assert!(parse_machine_connections(r#"{"Name":"m"}"#).is_empty());
    }

    /// Integration: a container labeled with a dead pid is swept; one labeled
    /// with our own (live) pid survives.
    /// `FLETCH_PODMAN_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Podman; opt in via FLETCH_PODMAN_TESTS=1"]
    fn sweeps_dead_instance_containers_only() {
        if !crate::sandbox::podman::podman_tests_enabled() {
            return;
        }
        let run = |label: &str, name: &str| {
            let out = cli::run_podman(
                &[
                    "run", "-d", "--label", label, "--name", name, "busybox", "sleep", "60",
                ],
                Duration::from_secs(120),
            )
            .unwrap();
            assert!(
                out.status.success(),
                "podman run failed: {}",
                String::from_utf8_lossy(&out.stderr),
            );
        };
        // 99999998 exceeds macOS's pid range and can't be a live process.
        let dead_name = format!("fletch-test-dead-{}", std::process::id());
        let live_name = format!("fletch-test-live-{}", std::process::id());
        run(&format!("{HOST_PID_LABEL}=99999998"), &dead_name);
        run(&host_pid_label(), &live_name);

        let removed = sweep_orphans().unwrap();
        assert!(removed >= 1, "the dead-pid container should be swept");

        let exists = |name: &str| {
            let out = cli::run_podman(
                &["ps", "-aq", "--filter", &format!("name={name}")],
                Duration::from_secs(10),
            )
            .unwrap();
            !String::from_utf8_lossy(&out.stdout).trim().is_empty()
        };
        assert!(!exists(&dead_name), "dead-instance container must be gone");
        assert!(exists(&live_name), "live-instance container must survive");

        let _ = cli::run_podman(&["rm", "-f", &live_name], Duration::from_secs(30));
    }
}
