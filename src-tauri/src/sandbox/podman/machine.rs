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
//! exist there), as is a connection list we couldn't read after a retry — not
//! knowing where the run goes is no reason to drop the check. It is skipped only
//! when there is genuinely nothing to check: no machine at all (a native Linux
//! host or a local socket runs containers directly) or an inspect that reported
//! no mounts, where guessing "unshared" would refuse launches that work fine.
//!
//! The same resolution names the connection the launch is *pinned* to
//! ([`LaunchTarget`]), which is what makes the check and the run agree: reading
//! the state here and letting `podman run` pick its own target later is how a
//! validated path ends up mounting empty somewhere else.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::policy::resolve_existing_prefix;

use super::cli;

/// `podman machine inspect` is a local config read; this bound only reaps a
/// wedged invocation.
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Where one launch's podman invocations go, and what its mounts must live
/// under to be visible there. Resolved once per launch and carried through the
/// container's whole life, so validate, run, kill and reap all speak to the
/// same endpoint.
pub(super) struct LaunchTarget {
    /// The connection to pin every podman invocation for this container to.
    /// `None` = no connections configured (bare native podman), where there is
    /// no endpoint to name and the default is the only one.
    pub(super) connection: Option<String>,
    /// Shared host dirs to validate mount sources against. `None` = no VM in
    /// the path, or the question is unanswerable, so the check is skipped —
    /// guessing "unshared" would refuse launches that work.
    shared_dirs: Option<Vec<PathBuf>>,
}

/// Resolve the target for one launch: which connection `podman run` must be
/// pinned to, and that connection's shared host dirs.
///
/// Deliberately uncached, and re-run on every launch. The two reads are local
/// config (`podman system connection list`, `podman machine inspect`) and
/// negligible next to a container start, while a process-wide cache retains
/// roots from a connection or machine the next launch no longer uses — a
/// reviewed bug, not a hypothetical. A TTL cache can come back if the cost ever
/// shows up under a spawn storm.
pub(super) fn resolve_launch_target() -> Result<LaunchTarget> {
    // The machine the launch will actually go through, pinned by name from here
    // on — never a union across machines and never a "some other machine"
    // fallback: mounts of a machine this launch does not use can pass a path
    // that then arrives empty in the one it does.
    match connection_target() {
        ConnectionTarget::Machine { connection } => {
            let shared_dirs = machine_shared_dirs(&connection);
            Ok(LaunchTarget {
                connection: Some(connection),
                shared_dirs,
            })
        }
        // A socket endpoint still gets pinned — the default connection can
        // change mid-run, and teardown has to reach the container's own
        // endpoint — but there is no VM, so every host path is reachable.
        ConnectionTarget::LocalSocket { connection } => Ok(LaunchTarget {
            connection: Some(connection),
            shared_dirs: None,
        }),
        ConnectionTarget::NoDefault => {
            tracing::debug!(
                target: "fletch::podman",
                "podman default connection names no machine; skipping the shared-path preflight",
            );
            Ok(LaunchTarget {
                connection: None,
                shared_dirs: None,
            })
        }
        // Not the same as "no connections": we don't know what the run would go
        // through, so we can't validate its mounts. Refuse instead of quietly
        // dropping both the pin and the preflight.
        ConnectionTarget::Unreadable => Err(Error::SandboxUnavailable(
            "couldn't read podman's connection list, so the launch can't be validated against \
             the machine it would run in — the agent's mounts (workspace, RPC mailbox, \
             credentials) could arrive empty. Retry, or check `podman system connection list`."
                .to_string(),
        )),
        ConnectionTarget::Remote { connection } => Err(Error::SandboxUnavailable(format!(
            "podman's default connection `{connection}` targets a remote host, so the \
             agent's mounts (workspace, RPC mailbox, credentials) would not exist inside \
             its containers. Make a local Podman machine the default \
             (`podman system connection default <machine>`) before launching.",
        ))),
    }
}

/// The shared host dirs behind the connection `connection`, or `None` when the
/// question can't be answered (a stale connection naming a deleted machine, a
/// wedged CLI, or no mounts reported) — `podman run` will then fail on its own
/// terms.
///
/// Unpinned by design: `machine inspect` reads local machine config by name, and
/// the name is one of [`machine_candidates`].
fn machine_shared_dirs(connection: &str) -> Option<Vec<PathBuf>> {
    let inspected = machine_candidates(connection).into_iter().find_map(|name| {
        cli::run_podman(&["machine", "inspect", name], INSPECT_TIMEOUT)
            .ok()
            .filter(|out| out.status.success())
            .map(|out| (name, out))
    });
    let Some((machine, out)) = inspected else {
        tracing::debug!(
            target: "fletch::podman",
            connection = %connection,
            "podman machine inspect failed; skipping the shared-path preflight",
        );
        return None;
    };
    let dirs = parse_shared_dirs(&String::from_utf8_lossy(&out.stdout));
    if dirs.is_empty() {
        tracing::debug!(
            target: "fletch::podman",
            machine = %machine,
            "podman machine inspect reported no mounts; skipping the shared-path preflight",
        );
        return None;
    }
    tracing::info!(target: "fletch::podman", machine = %machine, ?dirs, "podman machine shared directories");
    Some(dirs)
}

/// Machine names to try `podman machine inspect` with, in order. Verbatim
/// first: a machine can genuinely be *named* `foo-root`, and stripping the
/// suffix outright would inspect another machine's mounts. The stripped form
/// follows for the rootful half of a `<machine>`/`<machine>-root` pair.
fn machine_candidates(connection: &str) -> Vec<&str> {
    let mut names = vec![connection];
    if let Some(stripped) = connection
        .strip_suffix("-root")
        .filter(|name| !name.is_empty())
    {
        names.push(stripped);
    }
    names
}

/// Refuse the launch when any of `sources` lies outside the shared directories
/// of `target` — the connection this launch is pinned to, so the dirs checked
/// are the dirs the run resolves against. `sources` is every host path the run
/// will bind-mount — see
/// [`run_args::mount_sources`](crate::sandbox::container::run_args::mount_sources).
pub(super) fn ensure_sources_are_shared(sources: &[PathBuf], target: &LaunchTarget) -> Result<()> {
    let Some(shared) = target.shared_dirs.as_deref() else {
        return Ok(());
    };
    // Both spellings of every share. The run binds the *literal* source path and
    // podman resolves it again inside the VM, so a source counts as shared only
    // when both readings land in the shares: `/opt/repos/api` symlinked into
    // `$HOME` isn't there in the VM, and `/tmp` is the VM's own tmpfs, not the
    // host's `/private/tmp` share.
    let mut roots: Vec<PathBuf> = Vec::new();
    for dir in shared {
        for root in [dir.clone(), resolve_existing_prefix(dir)] {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    let Some((outside, resolved)) = sources
        .iter()
        .map(|src| (src, resolve_existing_prefix(src)))
        .find(|(src, resolved)| {
            !is_under_any(src.as_path(), &roots) || !is_under_any(resolved, &roots)
        })
    else {
        return Ok(());
    };
    let listed = shared
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    // Name the resolved path too: "X is outside the shares" is unactionable when
    // the real cause is where X resolves to.
    let via = if resolved == *outside {
        String::new()
    } else {
        format!(" (it resolves to {})", resolved.display())
    };
    Err(Error::SandboxUnavailable(format!(
        "{}{via} is outside the Podman machine's shared directories ({listed}), so it would mount \
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

/// Where podman's default connection points.
#[derive(Debug, PartialEq, Eq)]
enum ConnectionTarget {
    /// A machine, named by its verbatim entry name (`work-vm-root`) — the name
    /// `--connection` takes. `podman machine inspect` takes a *machine* name,
    /// which is one of [`machine_candidates`].
    Machine { connection: String },
    /// A local unix socket (rootful Linux, say): no VM in the path, every
    /// host path is reachable, nothing to check — but still an endpoint worth
    /// pinning, hence the name.
    LocalSocket { connection: String },
    /// A remote host: bind sources don't exist there, refuse the launch.
    Remote { connection: String },
    /// The list ran and parsed but names no default (or is empty): a native
    /// Linux host or an install with no connections at all.
    NoDefault,
    /// The list wouldn't run, timed out, or didn't parse — the target is
    /// unknown, which is not the same as absent.
    Unreadable,
}

/// The default connection's target, retrying an unreadable listing once — a
/// wedged or racing CLI read is transient, and the answer decides between
/// pinning the launch and refusing it.
fn connection_target() -> ConnectionTarget {
    let target = read_connection_target();
    if target != ConnectionTarget::Unreadable {
        return target;
    }
    tracing::debug!(
        target: "fletch::podman",
        "podman connection list unreadable; retrying once",
    );
    read_connection_target()
}

fn read_connection_target() -> ConnectionTarget {
    let Ok(out) = cli::run_podman(
        &["system", "connection", "list", "--format", "json"],
        INSPECT_TIMEOUT,
    ) else {
        return ConnectionTarget::Unreadable;
    };
    if !out.status.success() {
        return ConnectionTarget::Unreadable;
    }
    classify_default_connection(&String::from_utf8_lossy(&out.stdout))
}

/// Classify the default entry of `podman system connection list --format json`.
/// The connection name is kept verbatim — pinning `--connection work-vm` when
/// the default is `work-vm-root` would silently move the run to the rootless
/// endpoint. `IsMachine: false` splits on the URI: a
/// `unix://` socket is this host (no VM, nothing to check), anything else —
/// `ssh://`, `tcp://`, or a URI we can't read — is treated as remote and
/// refused rather than guessed at. Older podman omits `IsMachine`; the name is
/// taken as a machine name, and the inspect that follows settles whether it
/// really is one.
fn classify_default_connection(stdout: &str) -> ConnectionTarget {
    let Ok(connections) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return ConnectionTarget::Unreadable;
    };
    let Some(default) = connections
        .iter()
        .find(|c| c.get("Default").and_then(|d| d.as_bool()) == Some(true))
    else {
        return ConnectionTarget::NoDefault;
    };
    let name = default
        .get("Name")
        .and_then(|n| n.as_str())
        .map(str::trim)
        .unwrap_or_default();
    // A default we found but can't name is a shape we don't understand, not an
    // absent default.
    if name.is_empty() {
        return ConnectionTarget::Unreadable;
    }
    if default.get("IsMachine").and_then(|m| m.as_bool()) == Some(false) {
        let uri = default.get("URI").and_then(|u| u.as_str()).unwrap_or("");
        return if uri.starts_with("unix://") {
            ConnectionTarget::LocalSocket {
                connection: name.to_string(),
            }
        } else {
            ConnectionTarget::Remote {
                connection: name.to_string(),
            }
        };
    }
    ConnectionTarget::Machine {
        connection: name.to_string(),
    }
}

/// Host-side sources from `podman machine inspect` output: a JSON array whose
/// **first** machine's `Mounts` entries carry the host path as `Source` (and
/// the in-VM path as `Target`). Only the first machine is read — the caller
/// inspects one machine by name (or podman's default), and any further array
/// entries would belong to machines this launch does not go through.
///
/// Only mounts whose `Target` equals `Source` count: Fletch binds identical
/// host paths (invariant 1), which exist in the VM only where the share is
/// mounted at its own host path — a `--volume /host/x:/vm/y` share would
/// validate binds that then mount empty.
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
        let target = mount.get("Target").and_then(|t| t.as_str()).unwrap_or("");
        let source = source.trim();
        if source.is_empty() || !same_path(source, target.trim()) {
            continue;
        }
        let path = PathBuf::from(source);
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// Component-wise path equality, so `/Users/ada/` and `/Users/ada` are one path.
fn same_path(a: &str, b: &str) -> bool {
    Path::new(a).components().eq(Path::new(b).components())
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
    /// runs in. A share remapped to another in-VM path counts for nothing
    /// either: our binds use the host path, which isn't what the VM mounted.
    #[test]
    fn parses_mount_sources_from_first_machine_only() {
        let stdout = r#"[
          {
            "Name": "podman-machine-default",
            "State": "running",
            "Mounts": [
              { "ReadOnly": false, "Source": "/Users/ada", "Tag": "vol0", "Target": "/Users/ada", "Type": "virtiofs" },
              { "ReadOnly": false, "Source": "/private/tmp/", "Tag": "vol1", "Target": "/private/tmp", "Type": "virtiofs" },
              { "ReadOnly": false, "Source": "/Volumes/data", "Tag": "vol2", "Target": "/mnt/data", "Type": "virtiofs" },
              { "Source": "  ", "Target": "/blank" },
              { "Source": "/no-target" },
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
    /// consult: the name is kept verbatim for the pin, a local unix socket means
    /// no VM at all, and a remote default must classify as `Remote` — never fall
    /// back to some local machine whose mounts say nothing about where this
    /// run's binds resolve.
    #[test]
    fn default_connection_classifies_machine_socket_and_remote() {
        let rootful = r#"[
          { "Name": "podman-machine-default", "IsMachine": true, "Default": false },
          { "Name": "work-vm-root", "IsMachine": true, "Default": true },
          { "Name": "work-vm", "IsMachine": true, "Default": false }
        ]"#;
        assert_eq!(
            classify_default_connection(rootful),
            ConnectionTarget::Machine {
                connection: "work-vm-root".to_string(),
            }
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
            ConnectionTarget::LocalSocket {
                connection: "local-root".to_string()
            }
        );

        // Older podman omits IsMachine; the name is taken as a machine name
        // and the inspect that follows settles it.
        let legacy = r#"[ { "Name": "podman-machine-default", "Default": true } ]"#;
        assert_eq!(
            classify_default_connection(legacy),
            ConnectionTarget::Machine {
                connection: "podman-machine-default".to_string(),
            }
        );
    }

    /// "No default" and "couldn't read the list" are different answers: the
    /// first is a genuine no-connections host and launches unpinned, the second
    /// is unknown and must reach the caller's retry-then-refuse path rather than
    /// silently disabling the preflight.
    #[test]
    fn no_default_and_unreadable_are_distinct() {
        assert_eq!(
            classify_default_connection("[]"),
            ConnectionTarget::NoDefault
        );
        assert_eq!(
            classify_default_connection(r#"[{ "Name": "m", "Default": false }]"#),
            ConnectionTarget::NoDefault
        );

        assert_eq!(
            classify_default_connection("Error: unknown"),
            ConnectionTarget::Unreadable
        );
        assert_eq!(
            classify_default_connection(""),
            ConnectionTarget::Unreadable
        );
        // A default we found but can't name is a shape we don't understand.
        assert_eq!(
            classify_default_connection(r#"[{ "Name": "  ", "Default": true }]"#),
            ConnectionTarget::Unreadable
        );
    }

    /// The inspect name is tried verbatim before the `-root`-stripped form: a
    /// machine genuinely named `foo-root` must not resolve to `foo`, whose
    /// mounts say nothing about where this run's binds land.
    #[test]
    fn machine_candidates_try_the_verbatim_name_first() {
        assert_eq!(
            machine_candidates("podman-machine-default-root"),
            ["podman-machine-default-root", "podman-machine-default"],
        );
        assert_eq!(machine_candidates("foo-root"), ["foo-root", "foo"]);
        assert_eq!(machine_candidates("work-vm"), ["work-vm"]);
        assert_eq!(machine_candidates("-root"), ["-root"]);
    }

    /// The check reads the target's own dirs, and a target with none (no VM, or
    /// an unanswerable inspect) passes everything through.
    #[test]
    fn the_check_follows_the_targets_shared_dirs() {
        let home = dirs::home_dir().unwrap();
        let shared = LaunchTarget {
            connection: Some("podman-machine-default-root".to_string()),
            shared_dirs: Some(vec![home.clone()]),
        };
        ensure_sources_are_shared(std::slice::from_ref(&home), &shared).unwrap();
        let outside = PathBuf::from("/fletch-definitely-not-shared");
        let err = ensure_sources_are_shared(std::slice::from_ref(&outside), &shared).unwrap_err();
        assert!(
            err.to_string().contains(&outside.display().to_string()),
            "the refusal must name the path: {err}",
        );

        let unanswerable = LaunchTarget {
            connection: None,
            shared_dirs: None,
        };
        ensure_sources_are_shared(&[outside], &unanswerable).unwrap();
    }

    /// The run binds the literal path and podman resolves it inside the VM, so
    /// both readings have to be shared. A source that merely *sits* in a share
    /// but resolves out of it mounts empty, and so does `/tmp` against a
    /// `/private/tmp` share — the VM's `/tmp` is its own tmpfs.
    #[test]
    fn both_the_literal_and_the_resolved_source_must_be_shared() {
        let td = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(td.path()).unwrap();
        let share = root.join("share");
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&share).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let target = LaunchTarget {
            connection: Some("podman-machine-default".to_string()),
            shared_dirs: Some(vec![share.clone()]),
        };

        // Both forms inside the share: nothing to refuse.
        let inside = share.join("repo");
        std::fs::create_dir_all(&inside).unwrap();
        ensure_sources_are_shared(&[inside], &target).unwrap();

        // Literal inside, resolved outside: the VM binds the literal path and
        // finds nothing there.
        let link = share.join("api");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let err = ensure_sources_are_shared(std::slice::from_ref(&link), &target).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(&link.display().to_string()), "{msg}");
        assert!(
            msg.contains(&elsewhere.display().to_string()),
            "the refusal must name where the path resolves to: {msg}",
        );

        // `/tmp` against a `/private/tmp` share: resolved is shared, literal is
        // the VM's own tmpfs.
        let tmp_share = LaunchTarget {
            connection: None,
            shared_dirs: Some(vec![PathBuf::from("/private/tmp")]),
        };
        let err =
            ensure_sources_are_shared(&[PathBuf::from("/tmp/fletch-x")], &tmp_share).unwrap_err();
        assert!(err.to_string().contains("/tmp/fletch-x"), "{err}");
        ensure_sources_are_shared(&[PathBuf::from("/private/tmp/fletch-x")], &tmp_share).unwrap();
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
