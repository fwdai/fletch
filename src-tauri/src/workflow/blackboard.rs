//! The per-run **blackboard**: a host-owned directory shared read-write into
//! every step agent's sandbox, plus the helpers that read and curate it.
//!
//! Layout (spec §8.1), rooted at `~/.fletch/runs/<run-id>/`:
//!
//! ```text
//! ~/.fletch/runs/<run-id>/
//!   blackboard/
//!     task.md                 # engine-written: the run task + spec summary
//!     <step-id>/handoff.md    # agent-written free-form notes for downstream
//!     <step-id>/verdict.json  # agent-written structured completion signal
//!     shared/                 # agent-written cross-agent scratch space
//!   export/                   # reserved: journal.jsonl export (derived)
//! ```
//!
//! This module owns four things: **provisioning** the directory and writing
//! `task.md`; a **defensive `verdict.json` reader** that never panics and
//! distinguishes missing from malformed (both are gate-unmet, but the
//! re-prompt quotes the parse error); the **stale-verdict archival** helper
//! that keeps a leftover verdict from a previous loop iteration or retry from
//! satisfying a fresh gate (spec §8.3); and the **foreign-write scan** that
//! lets the engine journal a note when a step wrote outside its own lane
//! (spec §8.4 — prompt-enforced ownership, this is the detector).
//!
//! The sandbox *grant* that makes the blackboard writable from inside a step
//! agent is plumbed separately through [`crate::sandbox`] (seatbelt subpath /
//! Docker bind mount + the [`WF_BLACKBOARD_ENV`] env var); this module only
//! computes the paths.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Env var naming the blackboard directory inside a step agent's environment.
/// Both sandbox engines set it to the path the agent should read/write (the
/// same host path under either engine — Docker bind-mounts the blackboard at
/// its identical host path, mirroring the RPC mailbox mount). The step-protocol
/// prompt (S4) points the agent at `$WF_BLACKBOARD`.
pub const WF_BLACKBOARD_ENV: &str = "WF_BLACKBOARD";

/// Env var overriding the runs root (default `~/.fletch/runs`). Mirrors
/// [`crate::workspace::WORKSPACES_ROOT_ENV`]: a nested Fletch launched as a Run
/// process can't write the host's `~/.fletch`, so it is redirected. Set and
/// non-empty to override.
pub const RUNS_ROOT_ENV: &str = "FLETCH_RUNS_ROOT";

/// Absolute path to the root holding every run's host-owned directory:
/// `~/.fletch/runs/`. `$FLETCH_RUNS_ROOT` overrides it when set and non-empty.
pub fn runs_root() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os(RUNS_ROOT_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    Ok(home.join(".fletch").join("runs"))
}

/// Reject an id that is not a single safe path component. Run ids and step ids
/// are joined onto host paths that become sandbox *write grants*, so a
/// traversal component (`../other`) would place the grant outside the run
/// directory or a step's lane, and `.` would alias a whole parent directory.
/// Defense in depth: run ids are engine-generated and step ids are
/// spec-validated (S2), but these path-deriving helpers must not trust them.
fn validate_id(kind: &str, id: &str) -> Result<()> {
    let unsafe_component = id.is_empty()
        || id == "."
        || id == ".."
        || id.contains('/')
        || id.contains('\\')
        || id.contains('\0');
    if unsafe_component {
        return Err(Error::InvalidPath(format!(
            "unsafe workflow {kind} id: {id:?}"
        )));
    }
    Ok(())
}

/// `~/.fletch/runs/<run-id>/` — one run's host-owned directory (blackboard +,
/// once S4 lands, the run repository and journal export). Errors if `run_id`
/// is not a safe path component ([`validate_id`]).
pub fn run_dir(run_id: &str) -> Result<PathBuf> {
    validate_id("run", run_id)?;
    Ok(runs_root()?.join(run_id))
}

/// `<run-dir>/blackboard/` — the directory granted write access into each step
/// agent's sandbox. This is the value handed to the sandbox layer and exported
/// as [`WF_BLACKBOARD_ENV`].
pub fn blackboard_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("blackboard")
}

/// `<run-dir>/export/` — reserved for a derived `journal.jsonl` export.
pub fn export_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("export")
}

/// One step's blackboard subdirectory: `<blackboard>/<step-id>/`. A step owns
/// this directory and `shared/`; it reads everything. Errors if `step_id` is
/// not a safe path component ([`validate_id`]) — otherwise a step id like
/// `../other` would point outside the step's lane.
pub fn step_dir(blackboard: &Path, step_id: &str) -> Result<PathBuf> {
    validate_id("step", step_id)?;
    Ok(blackboard.join(step_id))
}

/// Provision the run directory layout and write `task.md`, returning the
/// blackboard path (the sandbox grant path). Idempotent: re-provisioning an
/// existing run dir re-creates any missing subdirs and rewrites `task.md`, so
/// resume after a restart is safe. Step subdirectories are created lazily by
/// the agents themselves (they hold the write grant); only `shared/`, the
/// blackboard root, and `export/` are provisioned here.
///
/// `task_md` is the full contents of `task.md` — the engine (S4) composes it
/// from the run task and a spec summary; this module just persists it.
pub fn provision(run_dir: &Path, task_md: &str) -> Result<PathBuf> {
    let blackboard = blackboard_dir(run_dir);
    std::fs::create_dir_all(blackboard.join("shared"))?;
    std::fs::create_dir_all(export_dir(run_dir))?;
    std::fs::write(blackboard.join("task.md"), task_md)?;
    Ok(blackboard)
}

/// The structured completion signal an agent writes to
/// `<step-id>/verdict.json` (spec §8.3). `deny_unknown_fields` keeps the shape
/// narrow: an unexpected key (a typo'd field name, a prompt-injected extra)
/// makes the verdict `Malformed` rather than being silently dropped, and the
/// re-prompt (§6.5) quotes the offending field back to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verdict {
    pub result: VerdictResult,
    /// One-line handoff for the timeline and the next agent. Defaulted so a
    /// verdict carrying only `result` still parses — the gate reads `result`,
    /// and a missing summary is a quality issue, not a malformed verdict.
    #[serde(default)]
    pub summary: String,
    /// Optional; e.g. structured review feedback.
    #[serde(default)]
    pub detail: Option<String>,
    /// Optional step-id to revise (loops only; must be inside the same loop).
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictResult {
    Done,
    Revise,
    Blocked,
}

/// Why reading a verdict did not yield a usable [`Verdict`]. Both variants mean
/// "gate unmet"; they differ only in what the engine journals and re-prompts:
/// a missing verdict is silent (the agent simply isn't done), a malformed one
/// carries the parse error so the re-prompt can quote it back (spec §8.3, §6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerdictError {
    /// No `verdict.json` at the expected path.
    Missing,
    /// The file exists but could not be read or parsed into a [`Verdict`]. The
    /// string is a human-readable cause suitable for a journal note and a
    /// re-prompt.
    Malformed(String),
}

/// Read and parse `<step-dir>/verdict.json` defensively. Never panics and never
/// returns a generic error: a missing file is [`VerdictError::Missing`], any
/// read or parse failure is [`VerdictError::Malformed`] with a cause. The
/// caller treats both as an unmet gate (spec §8.3).
pub fn read_verdict(step_dir: &Path) -> std::result::Result<Verdict, VerdictError> {
    let path = step_dir.join("verdict.json");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(VerdictError::Missing),
        Err(e) => {
            return Err(VerdictError::Malformed(format!(
                "cannot read verdict.json: {e}"
            )))
        }
    };
    serde_json::from_slice::<Verdict>(&bytes)
        .map_err(|e| VerdictError::Malformed(format!("invalid verdict.json: {e}")))
}

/// Move an existing `<step-dir>/verdict.json` aside before a fresh attempt's
/// prompt is sent, so a leftover verdict from a previous iteration or retry
/// cannot satisfy the new gate (spec §8.3, the loop/retry staleness bug class).
/// The verdict is moved to `<step-dir>/history/attempt-<attempt>.iter-<iteration>.verdict.json`;
/// the history files double as the loop's feedback trail. Returns the archive
/// destination when a verdict was moved, or `None` when there was none (the
/// common first-attempt case). The caller journals the move.
///
/// `attempt` and `iteration` label the verdict being archived — i.e. the
/// attempt/iteration that *wrote* it, not the one about to run.
pub fn archive_stale_verdict(
    step_dir: &Path,
    attempt: u32,
    iteration: u32,
) -> Result<Option<PathBuf>> {
    let src = step_dir.join("verdict.json");
    match src.symlink_metadata() {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Io(e)),
    }
    let history = step_dir.join("history");
    std::fs::create_dir_all(&history)?;
    // The same attempt can archive more than once (a blocked gate's re-prompt
    // archives again with the same labels) — never overwrite an earlier file.
    let mut dest = history.join(format!("attempt-{attempt}.iter-{iteration}.verdict.json"));
    let mut seq = 1;
    while dest.exists() {
        seq += 1;
        dest = history.join(format!(
            "attempt-{attempt}.iter-{iteration}.{seq}.verdict.json"
        ));
    }
    std::fs::rename(&src, &dest)?;
    Ok(Some(dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn run_dir_rejects_traversal_ids() {
        for bad in ["../other", "a/b", "..", ".", "", "a\\b"] {
            assert!(
                run_dir(bad).is_err(),
                "run id {bad:?} must be rejected as unsafe"
            );
        }
        // A plain component resolves under the runs root.
        assert!(run_dir("run-1").expect("safe id").ends_with("run-1"));
    }

    #[test]
    fn step_dir_rejects_traversal_and_dot() {
        let board = Path::new("/tmp/board");
        for bad in ["../other-run", ".", "..", "a/b", ""] {
            assert!(
                step_dir(board, bad).is_err(),
                "step id {bad:?} must be rejected as unsafe"
            );
        }
        assert_eq!(
            step_dir(board, "review").expect("safe id"),
            board.join("review")
        );
    }

    #[test]
    fn provision_creates_layout_and_writes_task() {
        let dir = tmp();
        let run = dir.path().join("run-1");
        let board = provision(&run, "the task\n\nsummary").expect("provision");

        assert_eq!(board, run.join("blackboard"));
        assert!(board.is_dir());
        assert!(board.join("shared").is_dir());
        assert!(run.join("export").is_dir());
        assert_eq!(
            std::fs::read_to_string(board.join("task.md")).unwrap(),
            "the task\n\nsummary"
        );
    }

    #[test]
    fn provision_is_idempotent_and_rewrites_task() {
        let dir = tmp();
        let run = dir.path().join("run-1");
        provision(&run, "first").expect("provision");
        // A step wrote a file; re-provisioning (resume) must not wipe it.
        let board = run.join("blackboard");
        std::fs::create_dir_all(board.join("plan")).unwrap();
        std::fs::write(board.join("plan/handoff.md"), "notes").unwrap();

        provision(&run, "second").expect("re-provision");
        assert_eq!(
            std::fs::read_to_string(board.join("task.md")).unwrap(),
            "second"
        );
        assert_eq!(
            std::fs::read_to_string(board.join("plan/handoff.md")).unwrap(),
            "notes"
        );
    }

    #[test]
    fn read_verdict_missing_is_typed() {
        let dir = tmp();
        assert_eq!(read_verdict(dir.path()), Err(VerdictError::Missing));
    }

    #[test]
    fn read_verdict_malformed_carries_cause() {
        let dir = tmp();
        std::fs::write(dir.path().join("verdict.json"), "{ not json").unwrap();
        match read_verdict(dir.path()) {
            Err(VerdictError::Malformed(msg)) => assert!(msg.contains("invalid verdict.json")),
            other => panic!("expected malformed, got {other:?}"),
        }
    }

    #[test]
    fn read_verdict_valid_parses_all_fields() {
        let dir = tmp();
        std::fs::write(
            dir.path().join("verdict.json"),
            r#"{"result":"revise","summary":"needs work","detail":"fix x","target":"review"}"#,
        )
        .unwrap();
        let v = read_verdict(dir.path()).expect("valid verdict");
        assert_eq!(v.result, VerdictResult::Revise);
        assert_eq!(v.summary, "needs work");
        assert_eq!(v.detail.as_deref(), Some("fix x"));
        assert_eq!(v.target.as_deref(), Some("review"));
    }

    #[test]
    fn read_verdict_accepts_result_only() {
        let dir = tmp();
        std::fs::write(dir.path().join("verdict.json"), r#"{"result":"done"}"#).unwrap();
        let v = read_verdict(dir.path()).expect("result-only verdict");
        assert_eq!(v.result, VerdictResult::Done);
        assert_eq!(v.summary, "");
        assert!(v.detail.is_none());
    }

    #[test]
    fn read_verdict_rejects_unknown_fields() {
        let dir = tmp();
        std::fs::write(
            dir.path().join("verdict.json"),
            r#"{"result":"done","summary":"ok","unexpected":"x"}"#,
        )
        .unwrap();
        match read_verdict(dir.path()) {
            Err(VerdictError::Malformed(msg)) => assert!(msg.contains("unexpected")),
            other => panic!("expected malformed on unknown field, got {other:?}"),
        }
    }

    #[test]
    fn read_verdict_rejects_unknown_result() {
        let dir = tmp();
        std::fs::write(dir.path().join("verdict.json"), r#"{"result":"maybe"}"#).unwrap();
        assert!(matches!(
            read_verdict(dir.path()),
            Err(VerdictError::Malformed(_))
        ));
    }

    #[test]
    fn archive_stale_verdict_moves_and_labels() {
        let dir = tmp();
        let step = dir.path();
        std::fs::write(step.join("verdict.json"), r#"{"result":"revise"}"#).unwrap();

        let dest = archive_stale_verdict(step, 1, 0).expect("archive").unwrap();
        assert_eq!(
            dest,
            step.join("history").join("attempt-1.iter-0.verdict.json")
        );
        assert!(!step.join("verdict.json").exists());
        assert!(dest.is_file());
    }

    #[test]
    fn archive_stale_verdict_never_overwrites_same_labels() {
        // A blocked gate's re-prompt archives again with the same attempt/
        // iteration labels; the second archive must not replace the first.
        let dir = tmp();
        let step = dir.path();
        std::fs::write(step.join("verdict.json"), r#"{"result":"revise"}"#).unwrap();
        let first = archive_stale_verdict(step, 1, 0).expect("archive").unwrap();

        std::fs::write(step.join("verdict.json"), r#"{"result":"blocked"}"#).unwrap();
        let second = archive_stale_verdict(step, 1, 0).expect("archive").unwrap();

        assert_ne!(first, second);
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            r#"{"result":"revise"}"#
        );
        assert_eq!(
            std::fs::read_to_string(&second).unwrap(),
            r#"{"result":"blocked"}"#
        );
    }

    #[test]
    fn archive_stale_verdict_noop_when_absent() {
        let dir = tmp();
        assert_eq!(
            archive_stale_verdict(dir.path(), 2, 3).expect("archive"),
            None
        );
    }

    #[test]
    fn archived_verdict_does_not_satisfy_a_new_gate() {
        // The staleness bug: a leftover done-verdict must not survive to the
        // next iteration's gate. After archival, the step dir has no verdict.
        let dir = tmp();
        let step = dir.path();
        std::fs::write(step.join("verdict.json"), r#"{"result":"done"}"#).unwrap();
        archive_stale_verdict(step, 1, 0).expect("archive");
        assert_eq!(read_verdict(step), Err(VerdictError::Missing));
    }
}
