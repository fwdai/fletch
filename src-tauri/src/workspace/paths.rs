//! Checkout filesystem layout and one-time root migration.

use super::*;

/// Compute a unique subdir name for a new tracked repo. Basename of
/// the repo path, with `-2`, `-3`, … suffix appended on collision with
/// an existing subdir in the same agent.
pub fn allocate_repo_subdir(repo_path: &Path, used: &[String]) -> String {
    let base = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo")
        .to_string();
    // `.fletch-profile` is reserved for Fletch-generated per-agent artifacts
    // (skill files, MCP config — see `agent_profile::PROFILE_DIR`); a repo with
    // that basename gets a numbered subdir instead of colliding with it.
    let reserved = base == crate::agent_profile::PROFILE_DIR;
    if !reserved && !used.iter().any(|u| u == &base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !used.iter().any(|u| u == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Env var overriding the checkouts root (default `~/.fletch/workspaces`). The
/// Run sandbox forbids writes to the host's `~/.fletch/workspaces`, so a nested
/// Fletch launched as a Run process (dogfooding: Fletch running Fletch) is
/// pointed at a sandbox-writable root instead — see
/// `sandbox::nested_checkouts_root`. Mirrors `rpc::RPC_ROOT_ENV`.
pub const WORKSPACES_ROOT_ENV: &str = "FLETCH_WORKSPACES_ROOT";

/// Absolute path to the root holding this build's agent checkouts —
/// `~/.fletch/workspaces/` (release) or `~/.fletch/dev/workspaces/` (debug).
///
/// The per-build split is what lets name allocation be DB-authoritative: each
/// build gets an *exclusive* root, so a name freed in one build's DB can be
/// reused without the filesystem colliding with another build, and provision
/// can safely clear a leftover dir (it can only ever be this build's own
/// crash-orphan). `$FLETCH_WORKSPACES_ROOT` redirects the *base* (nested-Fletch
/// Run, whose sandbox denies the host's `~/.fletch`) — but the build subpath is
/// still appended, so the override can't bypass the split: two different builds
/// pointed at the same override still land in separate roots, preserving that
/// exclusivity invariant.
pub fn checkouts_root() -> Result<PathBuf> {
    let base = match std::env::var_os(WORKSPACES_ROOT_ENV).filter(|v| !v.is_empty()) {
        Some(root) => PathBuf::from(root),
        None => dirs::home_dir()
            .ok_or_else(|| Error::Other("HOME directory not available".into()))?
            .join(".fletch"),
    };
    Ok(checkouts_root_in(&base))
}

/// Apply the per-build split to a checkouts base. Split out so it's testable
/// without mutating the process-global override env var — and to keep the
/// override and default paths sharing the exact same "append the build subpath"
/// step, so neither can drift into bypassing the split.
pub(super) fn checkouts_root_in(base: &Path) -> PathBuf {
    base.join(build_workspaces_subpath())
}

/// The workspaces path segment(s) under the checkouts base (`~/.fletch` or the
/// `$FLETCH_WORKSPACES_ROOT` override), split per build so a debug instance
/// never shares the checkout namespace with a release install. Release
/// keeps the historical flat `workspaces/` (so existing installs need no
/// migration); debug builds get a sibling `dev/workspaces/` root, mirroring the
/// `dev` split `data_dir` already uses for app data.
pub(super) fn build_workspaces_subpath() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from("dev").join("workspaces")
    } else {
        PathBuf::from("workspaces")
    }
}

/// Env var overriding the tools root (default `~/.fletch/tools`). Same style as
/// [`WORKSPACES_ROOT_ENV`] — set in tests so the codegraph install/probe never
/// touches a developer's real `~/.fletch`.
pub const TOOLS_ROOT_ENV: &str = "FLETCH_TOOLS_ROOT";

/// Absolute path to the root holding Fletch-managed external tool installs:
/// `~/.fletch/tools/`. A sibling of the workspaces root, deliberately **outside**
/// the app-data dir (`~/Library/Application Support/com.fletch.desktop`) which
/// the sandbox policy denies sandboxed agents from reading — agents must be able
/// to exec the codegraph binary that lands here. `$FLETCH_TOOLS_ROOT` overrides
/// it when set and non-empty.
pub fn tools_root() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os(TOOLS_ROOT_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    Ok(home.join(".fletch").join("tools"))
}

/// Env var overriding the projects root (default `~/.fletch/projects`).
pub const PROJECTS_ROOT_ENV: &str = "FLETCH_PROJECTS_ROOT";

/// Absolute path to the root holding per-project Fletch state that isn't a
/// live agent checkout — currently the codegraph index mirrors at
/// `~/.fletch/projects/<project_id>/codegraph/<repo>/`. A sibling of the
/// workspaces root; `$FLETCH_PROJECTS_ROOT` overrides it when set and non-empty.
pub fn projects_root() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os(PROJECTS_ROOT_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    Ok(home.join(".fletch").join("projects"))
}

/// Absolute path to the dir holding all of one agent's checkouts:
/// `~/.fletch/workspaces/<agent-id>/`.
pub fn agent_parent_dir(agent_id: &str) -> Result<PathBuf> {
    Ok(checkouts_root()?.join(agent_id))
}

/// Absolute path to one tracked repo's checkout:
/// `~/.fletch/workspaces/<agent-id>/<subdir>/`.
pub fn repo_checkout_path(agent_id: &str, subdir: &str) -> Result<PathBuf> {
    Ok(agent_parent_dir(agent_id)?.join(subdir))
}

/// One-time rename of the legacy on-disk root. The provisioned-checkout root
/// used to live at `~/.fletch/worktrees`; the default is now
/// `~/.fletch/workspaces`. Existing installs still have their checkouts under
/// the old path, so move them once at startup. Best-effort and non-fatal: only
/// runs when the root isn't overridden by `WORKSPACES_ROOT_ENV`, only when the
/// old dir exists and the new one doesn't, and any error is logged and swallowed
/// so a failed move never blocks launch.
pub fn migrate_default_checkouts_root() {
    // An explicit override means the caller manages the location themselves
    // (e.g. the nested-Fletch Run redirect) — don't touch anything.
    let overridden = std::env::var_os(WORKSPACES_ROOT_ENV)
        .filter(|v| !v.is_empty())
        .is_some();
    let Some(home) = dirs::home_dir() else {
        return;
    };
    migrate_checkouts_root_in(&home.join(".fletch"), overridden);
}

/// Testable core of [`migrate_default_checkouts_root`]: within `fletch_dir`
/// (i.e. `~/.fletch`), rename the legacy `worktrees` root to `workspaces`.
/// No-ops when the root is overridden, when the legacy dir is absent, or when
/// the new dir already exists (never merges into a live root). Errors are
/// logged and swallowed so a failed move never blocks launch.
pub(super) fn migrate_checkouts_root_in(fletch_dir: &Path, overridden: bool) {
    if overridden {
        return;
    }
    let old = fletch_dir.join("worktrees");
    let new = fletch_dir.join("workspaces");
    if old.is_dir() && !new.exists() {
        match std::fs::rename(&old, &new) {
            Ok(()) => tracing::info!(
                old = %old.display(),
                new = %new.display(),
                "migrated legacy checkouts root to workspaces",
            ),
            Err(e) => tracing::warn!(
                old = %old.display(),
                new = %new.display(),
                error = %e,
                "failed to migrate legacy checkouts root; continuing",
            ),
        }
    }
}
