//! The dead-instance orphan sweep, the per-agent container removal, and the
//! stale-image GC, for podman.
//!
//! Same questions and same labels as [`docker::cleanup`](crate::sandbox::docker),
//! sharing its selection rule and under-reclaim bias
//! ([`container::image_gc`](crate::sandbox::container::image_gc)); only the
//! invocations differ. Podman has one container store *and* image store per
//! machine endpoint (a machine's rootless and rootful connections are two), and
//! launches pin to the connection they resolved at the time, so every sweep
//! runs once per machine connection ([`across_machines`]).
//!
//! The image GC runs the labeled arm only: docker's legacy arm covers images
//! built before the `fletch.agent` label, which no Podman store can hold.

use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::container::image_gc::{
    current_tags, image_removal_refs, known_repos, parse_images_line, ImageRow, IMAGES_FORMAT,
    RETIRED_REPOS,
};
use crate::sandbox::container::images::AGENT_IMAGE_LABEL;
use crate::sandbox::container::labels::{
    agent_id_filter, orphaned_ids, HOST_PID_LABEL, INSPECT_FORMAT,
};

use super::cli;

/// Listing/inspect are metadata-only; tripping this means the machine
/// connection is wedged.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// `podman rm -f` also kills a still-running container's process; room for the
/// batched removal without letting a hung machine pin the sweep thread.
const REMOVE_TIMEOUT: Duration = Duration::from_secs(60);

/// Remove every fletch-labeled container whose owning host instance is dead, on
/// every machine — yesterday's containers can sit on a machine that is no
/// longer the default. Returns the number removed. Callers gate on the probe
/// and run this off the main thread — see [`super::sweep_orphans_at_startup`].
pub(super) fn sweep_orphans() -> Result<usize> {
    across_machines("podman orphan sweep", sweep_orphans_on)
}

fn sweep_orphans_on(connection: Option<&str>) -> Result<usize> {
    let ids = list_ids(connection, &format!("label={HOST_PID_LABEL}"))?;
    if ids.is_empty() {
        return Ok(0);
    }

    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut inspect_args = vec!["inspect", "-f", INSPECT_FORMAT];
    inspect_args.extend(&id_refs);
    let inspected = cli::run_podman_on(connection, &inspect_args, QUERY_TIMEOUT)?;
    // No zero-exit requirement: inspect exits non-zero if ANY id vanished
    // between ps and inspect, but still prints the rows it found.
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

/// Remove every container labeled with this agent id, on every machine
/// endpoint. Returns the number removed. Disposal has decided, so no
/// pid-liveness check, and labels are the only stable handle — names carry a
/// launch nonce
/// ([`container::util::container_name`](crate::sandbox::container::util::container_name)).
pub fn remove_agent_containers(agent_id: &str) -> Result<usize> {
    across_machines("podman disposal removal", |connection| {
        remove_agent_containers_on(connection, agent_id)
    })
}

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
/// unpinned when there are none (native Linux).
///
/// Best-effort per connection: a wedged machine logs and the rest are still
/// swept. Total failure is not laundered into `Ok(0)` — with no connection
/// answering, the caller hears the last error.
fn across_machines(
    what: &str,
    mut pass: impl FnMut(Option<&str>) -> Result<usize>,
) -> Result<usize> {
    let connections = machine_connections();
    if connections.is_empty() {
        return pass(None);
    }
    let mut total = 0;
    let mut succeeded = false;
    let mut last_error = None;
    for connection in &connections {
        match pass(Some(connection)) {
            Ok(n) => {
                succeeded = true;
                total += n;
            }
            Err(e) => {
                tracing::warn!(
                    target: "fletch::podman",
                    connection = %connection,
                    error = %e,
                    "{what} failed on this connection",
                );
                last_error = Some(e);
            }
        }
    }
    match last_error {
        Some(e) if !succeeded => Err(e),
        _ => Ok(total),
    }
}

/// One connection name per Podman machine. Anything we can't run or read yields
/// none, which the caller reads as "just use the default".
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

/// The machine connection names in a listing: entries not explicitly
/// `IsMachine: false` (older podman omits the field). A machine's `<machine>`
/// and `<machine>-root` entries are both kept — disjoint stores, so collapsing
/// the pair leaves one swept by nobody; only a shared `URI` is one endpoint.
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
        // No URI can't be deduped against: sweep twice rather than skip a store.
        let uri = connection
            .get("URI")
            .and_then(|u| u.as_str())
            .map(str::trim)
            .unwrap_or_default();
        if !uri.is_empty() {
            if seen.contains(&uri) {
                continue;
            }
            seen.push(uri);
        }
        out.push(name.to_string());
    }
    out
}

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

/// Remove `fletch.agent`-labeled images that aren't a current expected tag,
/// from every machine's store. Returns the number removed; callers treat
/// failures as non-fatal.
///
/// Never touched: current tags, the user's `podman_image` override, unlabeled
/// images, and images in use — `podman rmi` runs WITHOUT `-f`, so an in-use
/// image fails removal, which is expected and logged at debug.
pub(super) fn sweep_stale_images() -> Result<usize> {
    across_machines("podman image sweep", sweep_stale_images_on)
}

/// One image-GC pass against a single connection's store; a background refresh
/// rebuild calls this directly for the one store it just rebuilt in.
pub(super) fn sweep_stale_images_on(connection: Option<&str>) -> Result<usize> {
    let refs = image_removal_refs(
        &list_images(
            connection,
            &[
                "images",
                "--filter",
                &format!("label={AGENT_IMAGE_LABEL}"),
                "--format",
                IMAGES_FORMAT,
            ],
        )?,
        // No legacy arm: see the module doc — a Podman store predates nothing.
        &[],
        &current_tags(),
        &known_repos(),
        RETIRED_REPOS,
        &[],
        super::settings::image_override().as_deref(),
    );
    if refs.is_empty() {
        return Ok(0);
    }

    tracing::info!(
        count = refs.len(),
        "removing superseded fletch agent images from the podman store"
    );
    let mut removed = 0;
    for image_ref in &refs {
        // One rmi per image, not batched: a single in-use image or transport
        // failure must not taint the others' exit status or strand them.
        let out = match cli::run_podman_on(connection, &["rmi", image_ref], REMOVE_TIMEOUT) {
            Ok(out) => out,
            Err(e) => {
                tracing::warn!(
                    target: "fletch::podman",
                    image = %image_ref,
                    error = %e,
                    "podman rmi could not run for this image; continuing the pass",
                );
                continue;
            }
        };
        if out.status.success() {
            removed += 1;
        } else {
            tracing::debug!(
                target: "fletch::podman",
                image = %image_ref,
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "stale image not removed (expected when a container still uses it)",
            );
        }
    }
    Ok(removed)
}

fn list_images(connection: Option<&str>, args: &[&str]) -> Result<Vec<ImageRow>> {
    let out = cli::run_podman_on(connection, args, QUERY_TIMEOUT)?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "podman images failed: {}",
            String::from_utf8_lossy(&out.stderr).trim(),
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_images_line)
        .collect())
}

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

    /// One connection per *endpoint*: a rootless/rootful pair is two stores,
    /// a shared URI is one, and non-machine entries aren't ours to sweep.
    #[test]
    fn machine_connections_are_one_per_endpoint() {
        let listing = r#"[
          { "Name": "podman-machine-default", "IsMachine": true, "Default": true, "URI": "ssh://core@127.0.0.1:52001/run/user/501/podman/podman.sock" },
          { "Name": "podman-machine-default-root", "IsMachine": true, "Default": false, "URI": "ssh://root@127.0.0.1:52001/run/podman/podman.sock" },
          { "Name": "work-vm-alias", "IsMachine": true, "Default": false, "URI": "ssh://core@127.0.0.1:52001/run/user/501/podman/podman.sock" },
          { "Name": "work-vm", "IsMachine": true, "Default": false, "URI": "ssh://core@127.0.0.1:52002/run/user/501/podman/podman.sock" },
          { "Name": "build-box", "IsMachine": false, "Default": false, "URI": "ssh://core@build.example.com:22/run/podman/podman.sock" },
          { "Name": "local", "IsMachine": false, "Default": false, "URI": "unix:///run/podman/podman.sock" },
          { "Name": "  ", "IsMachine": true },
          { "IsMachine": true },
          { "Name": "legacy-no-flag" }
        ]"#;
        assert_eq!(
            parse_machine_connections(listing),
            [
                "podman-machine-default",
                "podman-machine-default-root",
                "work-vm",
                "legacy-no-flag",
            ],
        );

        assert!(parse_machine_connections("[]").is_empty());
        assert!(parse_machine_connections("").is_empty());
        assert!(parse_machine_connections("Error: no such thing").is_empty());
        assert!(parse_machine_connections(r#"{"Name":"m"}"#).is_empty());
    }

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

    /// Also pins the assumption the shared selection rule rests on: `podman
    /// images --format` prints the same three columns docker does, so
    /// [`parse_images_line`] reads both. Runs against the default connection
    /// (`None`), which is where the images it builds land.
    /// `FLETCH_PODMAN_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Podman; opt in via FLETCH_PODMAN_TESTS=1"]
    fn sweeps_stale_labeled_images_only() {
        if !crate::sandbox::podman::podman_tests_enabled() {
            return;
        }
        let build = |dockerfile: &str, tag: &str| {
            let ctx = tempfile::tempdir().unwrap();
            std::fs::write(ctx.path().join("Dockerfile"), dockerfile).unwrap();
            let out = cli::run_podman(
                &["build", "-t", tag, &ctx.path().to_string_lossy()],
                Duration::from_secs(120),
            )
            .unwrap();
            assert!(
                out.status.success(),
                "podman build failed: {}",
                String::from_utf8_lossy(&out.stderr),
            );
        };
        // A tag no provider owns: what a superseded image looks like.
        let stale_tag = "fletch-agent:000000000000";
        build(
            &format!("FROM busybox\nLABEL {AGENT_IMAGE_LABEL}=claude\n"),
            stale_tag,
        );
        // Unlabeled, outside Fletch's repos: must survive.
        let bystander = "fletch-gc-test-bystander:keep";
        build("FROM busybox\nENV FLETCH_GC_TEST=1\n", bystander);

        // The listing must parse before the sweep's verdict means anything.
        let rows = list_images(
            None,
            &[
                "images",
                "--filter",
                &format!("label={AGENT_IMAGE_LABEL}"),
                "--format",
                IMAGES_FORMAT,
            ],
        )
        .unwrap();
        // The normalized repo: podman prints local builds as
        // `localhost/fletch-agent`.
        assert!(
            rows.iter()
                .any(|r| r.compare_repo() == "fletch-agent" && r.tag == "000000000000"),
            "podman's --format output must parse into the shared row shape: {rows:?}",
        );

        let removed = sweep_stale_images_on(None).unwrap();
        assert!(removed >= 1, "the stale labeled image should be swept");

        let exists = |tag: &str| {
            cli::run_podman(&["image", "inspect", tag], Duration::from_secs(10))
                .unwrap()
                .status
                .success()
        };
        assert!(!exists(stale_tag), "stale labeled image must be gone");
        assert!(exists(bystander), "unlabeled non-fletch image must survive");

        let _ = cli::run_podman(&["rmi", "-f", bystander], Duration::from_secs(30));
    }
}
