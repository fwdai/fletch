//! The `podman machine` shared-directory preflight.
//!
//! Podman on macOS runs containers inside a Linux VM, and that VM sees only the
//! host directories the machine was configured to share (by default `$HOME`).
//! A bind mount whose source lies outside them does not fail — the VM has
//! nothing at that path, so the container gets an **empty directory**. Every
//! Fletch mount is at its identical host path (invariant 1), so an agent whose
//! workspace or RPC mailbox lands outside the shared set would start, find an
//! empty checkout and an unreachable mailbox, and fail in ways that look like
//! anything but a mount problem.
//!
//! So the launch is refused up front, naming the path and the shared dirs. The
//! dirs come from the machine behind podman's *default connection* — the one
//! `podman run` will use — never from any other machine, whose mounts say
//! nothing about where this run's binds resolve. A default connection that
//! targets a remote host is refused outright (the identical-path binds cannot
//! exist there); the check is skipped only when it genuinely can't be answered
//! (no machine at all — a native Linux host or a local socket runs containers
//! directly — or a connection list / inspect we couldn't read): guessing
//! "unshared" there would refuse launches that work fine.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::policy::resolve_existing_prefix;

use super::cli;

/// `podman machine inspect` is a local config read; this bound only reaps a
/// wedged invocation.
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Refuse the launch when any of `sources` lies outside the machine's shared
/// directories — or when podman's default connection targets a remote host,
/// where the identical-path mounts cannot exist at all. `sources` is every
/// host path the run will bind-mount — see
/// [`run_args::mount_sources`](crate::sandbox::container::run_args::mount_sources).
pub(super) fn ensure_sources_are_shared(sources: &[PathBuf]) -> Result<()> {
    let shared = match preflight() {
        Preflight::Skip => return Ok(()),
        Preflight::Remote { connection } => {
            return Err(Error::SandboxUnavailable(format!(
                "podman's default connection `{connection}` targets a remote host, so the \
                 agent's mounts (workspace, RPC mailbox, credentials) would not exist inside \
                 its containers. Make a local Podman machine the default \
                 (`podman system connection default <machine>`) before launching.",
            )));
        }
        Preflight::Dirs(dirs) => dirs,
    };
    let roots: Vec<PathBuf> = shared.iter().map(|p| resolve_existing_prefix(p)).collect();
    let Some(outside) = sources
        .iter()
        .find(|src| !is_under_any(&resolve_existing_prefix(src), &roots))
    else {
        return Ok(());
    };
    let listed = shared
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(Error::SandboxUnavailable(format!(
        "{} is outside the Podman machine's shared directories ({listed}), so it would mount \
         empty inside the container. Share it with `podman machine set --volume <dir>` and \
         restart the machine, or keep the agent's workspace under a shared directory.",
        outside.display(),
    )))
}

/// Whether `path` is at or below one of `roots`. Pure over already-resolved
/// paths — component-wise, so `/Users/ab` is not read as being under
/// `/Users/a`. An empty `roots` covers nothing, which is why the caller treats
/// "no shared dirs reported" as unanswerable rather than passing it through.
fn is_under_any(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

/// What the preflight resolved, read once per app run: the mounts don't
/// change without a `podman machine set` + restart, and the check runs on
/// every launch.
enum Preflight {
    /// The connection's machine answered with its shared host dirs — check
    /// every mount source against them.
    Dirs(Vec<PathBuf>),
    /// Unanswerable (no machine — native Linux or a local socket — an inspect
    /// or connection list we couldn't read, or no mounts reported): guessing
    /// "unshared" would refuse launches that work, so the check is skipped.
    Skip,
    /// The default connection targets a remote host. Not a mounts question:
    /// the identical-path binds cannot exist there at all, so the launch is
    /// refused — and never validated against some *local* machine's mounts,
    /// which say nothing about where this run's binds resolve.
    Remote { connection: String },
}

fn preflight() -> &'static Preflight {
    static P: OnceLock<Preflight> = OnceLock::new();
    P.get_or_init(|| {
        // Resolve the machine the launch will actually go through — the one
        // behind podman's default connection (Fletch never passes
        // `--connection`) — never a union across machines and never a "some
        // other machine" fallback: mounts of a machine this launch does not
        // use can pass a path that then arrives empty in the one it does.
        let machine = match connection_target() {
            ConnectionTarget::Machine(name) => name,
            ConnectionTarget::Remote { connection } => {
                return Preflight::Remote { connection };
            }
            ConnectionTarget::LocalSocket | ConnectionTarget::Unknown => {
                tracing::debug!(
                    target: "fletch::podman",
                    "podman default connection names no machine; skipping the shared-path preflight",
                );
                return Preflight::Skip;
            }
        };
        let inspect = cli::run_podman(&["machine", "inspect", &machine], INSPECT_TIMEOUT);
        let out = match inspect {
            Ok(out) if out.status.success() => out,
            // A stale connection naming a deleted machine, or a wedged CLI:
            // unanswerable, and `podman run` will fail on its own terms.
            _ => {
                tracing::debug!(
                    target: "fletch::podman",
                    machine = %machine,
                    "podman machine inspect failed; skipping the shared-path preflight",
                );
                return Preflight::Skip;
            }
        };
        let dirs = parse_shared_dirs(&String::from_utf8_lossy(&out.stdout));
        if dirs.is_empty() {
            tracing::debug!(
                target: "fletch::podman",
                machine = %machine,
                "podman machine inspect reported no mounts; skipping the shared-path preflight",
            );
            return Preflight::Skip;
        }
        tracing::info!(target: "fletch::podman", machine = %machine, ?dirs, "podman machine shared directories");
        Preflight::Dirs(dirs)
    })
}

/// Where podman's default connection points.
#[derive(Debug, PartialEq, Eq)]
enum ConnectionTarget {
    /// A machine, by name — the authoritative source for shared dirs.
    Machine(String),
    /// A local unix socket (rootful Linux, say): no VM in the path, every
    /// host path is reachable, nothing to check.
    LocalSocket,
    /// A remote host: bind sources don't exist there, refuse the launch.
    Remote { connection: String },
    /// No default entry, or a list we couldn't run or read.
    Unknown,
}

fn connection_target() -> ConnectionTarget {
    let Ok(out) = cli::run_podman(
        &["system", "connection", "list", "--format", "json"],
        INSPECT_TIMEOUT,
    ) else {
        return ConnectionTarget::Unknown;
    };
    if !out.status.success() {
        return ConnectionTarget::Unknown;
    }
    classify_default_connection(&String::from_utf8_lossy(&out.stdout))
}

/// Classify the default entry of `podman system connection list --format json`.
/// Machine connections come in `<machine>` / `<machine>-root` pairs, so a
/// trailing `-root` is stripped. `IsMachine: false` splits on the URI: a
/// `unix://` socket is this host (no VM, nothing to check), anything else —
/// `ssh://`, `tcp://`, or a URI we can't read — is treated as remote and
/// refused rather than guessed at. Older podman omits `IsMachine`; the name is
/// taken as a machine name, and the inspect that follows settles whether it
/// really is one.
fn classify_default_connection(stdout: &str) -> ConnectionTarget {
    let Ok(connections) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return ConnectionTarget::Unknown;
    };
    let Some(default) = connections
        .iter()
        .find(|c| c.get("Default").and_then(|d| d.as_bool()) == Some(true))
    else {
        return ConnectionTarget::Unknown;
    };
    let name = default
        .get("Name")
        .and_then(|n| n.as_str())
        .map(str::trim)
        .unwrap_or_default();
    if name.is_empty() {
        return ConnectionTarget::Unknown;
    }
    if default.get("IsMachine").and_then(|m| m.as_bool()) == Some(false) {
        let uri = default.get("URI").and_then(|u| u.as_str()).unwrap_or("");
        return if uri.starts_with("unix://") {
            ConnectionTarget::LocalSocket
        } else {
            ConnectionTarget::Remote {
                connection: name.to_string(),
            }
        };
    }
    ConnectionTarget::Machine(name.strip_suffix("-root").unwrap_or(name).to_string())
}

/// Host-side sources from `podman machine inspect` output: a JSON array whose
/// **first** machine's `Mounts` entries carry the host path as `Source` (and
/// the in-VM path as `Target`). Only the first machine is read — the caller
/// inspects one machine by name (or podman's default), and any further array
/// entries would belong to machines this launch does not go through.
///
/// Tolerant by design: a shape we don't recognize yields an empty set, which
/// the caller reads as "unanswerable" and skips.
fn parse_shared_dirs(stdout: &str) -> Vec<PathBuf> {
    let Ok(machines) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = Vec::new();
    let Some(machine) = machines.first() else {
        return out;
    };
    let Some(mounts) = machine.get("Mounts").and_then(|m| m.as_array()) else {
        return out;
    };
    for mount in mounts {
        let Some(source) = mount.get("Source").and_then(|s| s.as_str()) else {
            continue;
        };
        let source = source.trim();
        if source.is_empty() {
            continue;
        }
        let path = PathBuf::from(source);
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Containment is component-wise: a sibling whose name merely starts with a
    /// shared dir's name is not inside it.
    #[test]
    fn containment_is_component_wise() {
        let roots = vec![PathBuf::from("/Users/ada"), PathBuf::from("/private/tmp")];

        assert!(is_under_any(Path::new("/Users/ada"), &roots));
        assert!(is_under_any(
            Path::new("/Users/ada/.fletch/worktrees/orkney"),
            &roots
        ));
        assert!(is_under_any(Path::new("/private/tmp/x"), &roots));

        assert!(!is_under_any(Path::new("/Users/adamant/repo"), &roots));
        assert!(!is_under_any(Path::new("/Volumes/ext/repo"), &roots));
        assert!(!is_under_any(Path::new("/Users"), &roots));
        // Nothing is shared when nothing is reported — which is why the caller
        // treats an empty set as unanswerable instead of refusing every launch.
        assert!(!is_under_any(Path::new("/Users/ada"), &[]));
    }

    /// The `podman machine inspect` shape: an array of machines, host paths in
    /// each mount's `Source`. Only the first machine counts — a mount shared
    /// only by a second machine must NOT pass the preflight, or the launch
    /// would proceed and bind an empty directory in the machine it actually
    /// runs in.
    #[test]
    fn parses_mount_sources_from_first_machine_only() {
        let stdout = r#"[
          {
            "Name": "podman-machine-default",
            "State": "running",
            "Mounts": [
              { "ReadOnly": false, "Source": "/Users/ada", "Tag": "vol0", "Target": "/Users/ada", "Type": "virtiofs" },
              { "ReadOnly": false, "Source": "/private/tmp", "Tag": "vol1", "Target": "/private/tmp", "Type": "virtiofs" },
              { "Source": "  ", "Target": "/blank" },
              { "Target": "/no-source" }
            ]
          },
          {
            "Name": "second",
            "Mounts": [
              { "Source": "/Volumes/only-on-second", "Target": "/Volumes/only-on-second" }
            ]
          }
        ]"#;
        assert_eq!(
            parse_shared_dirs(stdout),
            vec![PathBuf::from("/Users/ada"), PathBuf::from("/private/tmp")],
        );
    }

    /// The default connection decides which machine (if any) the preflight may
    /// consult: `-root` pairs collapse to the machine name, a local unix socket
    /// means no VM at all, and a remote default must classify as `Remote` —
    /// never fall back to some local machine whose mounts say nothing about
    /// where this run's binds resolve.
    #[test]
    fn default_connection_classifies_machine_socket_and_remote() {
        let rootful = r#"[
          { "Name": "podman-machine-default", "IsMachine": true, "Default": false },
          { "Name": "work-vm-root", "IsMachine": true, "Default": true },
          { "Name": "work-vm", "IsMachine": true, "Default": false }
        ]"#;
        assert_eq!(
            classify_default_connection(rootful),
            ConnectionTarget::Machine("work-vm".to_string())
        );

        let remote_ssh = r#"[
          { "Name": "build-box", "IsMachine": false, "Default": true, "URI": "ssh://core@build.example.com:22/run/podman/podman.sock" },
          { "Name": "podman-machine-default", "IsMachine": true, "Default": false }
        ]"#;
        assert_eq!(
            classify_default_connection(remote_ssh),
            ConnectionTarget::Remote {
                connection: "build-box".to_string()
            }
        );

        // Explicitly not a machine with no readable URI: refused, not guessed.
        let remote_no_uri = r#"[ { "Name": "mystery", "IsMachine": false, "Default": true } ]"#;
        assert_eq!(
            classify_default_connection(remote_no_uri),
            ConnectionTarget::Remote {
                connection: "mystery".to_string()
            }
        );

        let local_socket = r#"[
          { "Name": "local-root", "IsMachine": false, "Default": true, "URI": "unix:///run/podman/podman.sock" }
        ]"#;
        assert_eq!(
            classify_default_connection(local_socket),
            ConnectionTarget::LocalSocket
        );

        // Older podman omits IsMachine; the name is taken as a machine name
        // and the inspect that follows settles it.
        let legacy = r#"[ { "Name": "podman-machine-default", "Default": true } ]"#;
        assert_eq!(
            classify_default_connection(legacy),
            ConnectionTarget::Machine("podman-machine-default".to_string())
        );

        assert_eq!(classify_default_connection("[]"), ConnectionTarget::Unknown);
        assert_eq!(
            classify_default_connection("Error: unknown"),
            ConnectionTarget::Unknown
        );
        assert_eq!(
            classify_default_connection(r#"[{ "Name": "m", "Default": false }]"#),
            ConnectionTarget::Unknown
        );
    }

    /// Anything we can't read yields an empty set, so the caller skips the
    /// check rather than refusing launches on a shape change.
    #[test]
    fn unrecognized_inspect_output_yields_no_dirs() {
        assert!(parse_shared_dirs("[]").is_empty());
        assert!(parse_shared_dirs("").is_empty());
        assert!(parse_shared_dirs("Error: no such machine").is_empty());
        assert!(parse_shared_dirs(r#"[{"Name":"m"}]"#).is_empty());
        assert!(parse_shared_dirs(r#"[{"Mounts":{}}]"#).is_empty());
    }
}
