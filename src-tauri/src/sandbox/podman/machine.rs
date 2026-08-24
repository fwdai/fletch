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
//! check is skipped whenever it can't be answered (no machine at all — a native
//! Linux host runs containers directly — or an inspect we couldn't parse):
//! guessing "unshared" there would refuse launches that work fine.

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
/// directories. `sources` is every host path the run will bind-mount — see
/// [`run_args::mount_sources`](crate::sandbox::container::run_args::mount_sources).
pub(super) fn ensure_sources_are_shared(sources: &[PathBuf]) -> Result<()> {
    let Some(shared) = shared_dirs() else {
        return Ok(());
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

/// The machine's shared host directories, read once per app run: the mounts
/// don't change without a `podman machine set` + restart, and the check runs on
/// every launch. `None` means "unanswerable" — no machine (native Linux), an
/// inspect that failed, or one reporting no mounts at all.
fn shared_dirs() -> Option<&'static Vec<PathBuf>> {
    static DIRS: OnceLock<Option<Vec<PathBuf>>> = OnceLock::new();
    DIRS.get_or_init(|| {
        let out = cli::run_podman(&["machine", "inspect"], INSPECT_TIMEOUT).ok()?;
        if !out.status.success() {
            // The usual case on Linux: `podman machine` reports no such
            // machine, because containers run on the host kernel directly and
            // every host path is reachable.
            tracing::debug!(
                target: "fletch::podman",
                "podman machine inspect reported no machine; skipping the shared-path preflight",
            );
            return None;
        }
        let dirs = parse_shared_dirs(&String::from_utf8_lossy(&out.stdout));
        if dirs.is_empty() {
            tracing::debug!(
                target: "fletch::podman",
                "podman machine inspect reported no mounts; skipping the shared-path preflight",
            );
            return None;
        }
        tracing::info!(target: "fletch::podman", ?dirs, "podman machine shared directories");
        Some(dirs)
    })
    .as_ref()
}

/// Host-side sources from `podman machine inspect` output: a JSON array of
/// machines, each with a `Mounts` array whose entries carry the host path as
/// `Source` (and the in-VM path as `Target`). Every machine's mounts are taken
/// — the connection a launch uses is whichever is current, and a path shared by
/// all of them is shared by that one too; taking the union only ever risks
/// letting a launch through, which is the same bias as skipping the check.
///
/// Tolerant by design: a shape we don't recognize yields an empty set, which
/// the caller reads as "unanswerable" and skips.
fn parse_shared_dirs(stdout: &str) -> Vec<PathBuf> {
    let Ok(machines) = serde_json::from_str::<Vec<serde_json::Value>>(stdout) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for machine in machines {
        let Some(mounts) = machine.get("Mounts").and_then(|m| m.as_array()) else {
            continue;
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
    /// each mount's `Source`, deduped across machines.
    #[test]
    fn parses_mount_sources_from_inspect_json() {
        let stdout = r#"[
          {
            "Name": "podman-machine-default",
            "State": "running",
            "Mounts": [
              { "ReadOnly": false, "Source": "/Users/ada", "Tag": "vol0", "Target": "/Users/ada", "Type": "virtiofs" },
              { "ReadOnly": false, "Source": "/private/tmp", "Tag": "vol1", "Target": "/private/tmp", "Type": "virtiofs" }
            ]
          },
          {
            "Name": "second",
            "Mounts": [
              { "Source": "/Users/ada", "Target": "/Users/ada" },
              { "Source": "  ", "Target": "/blank" },
              { "Target": "/no-source" }
            ]
          }
        ]"#;
        assert_eq!(
            parse_shared_dirs(stdout),
            vec![PathBuf::from("/Users/ada"), PathBuf::from("/private/tmp")],
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
