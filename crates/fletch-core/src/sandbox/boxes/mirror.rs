//! The host-side bare mirror of a box's checkout: `<box>/mirror.git`,
//! alternates pointing at the user's repository so only
//! the agent's own objects move, refreshed over the channel before anything
//! host-side needs the checkout's objects — publish and transport, which run
//! host git with the user's credentials and so must never run *in* the
//! checkout. Reads (`git_state`, diffs) do not come here: they run in the
//! guest through the routed seam.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fletch_box::Channel;

use crate::error::{Error, Result};
use crate::git;

use super::registry::{BoxRegistry, RunningBox, MIRROR_DIR};

/// A mirror fetch/push over vsock is local-fast; this is for a wedged guest.
const TIMEOUT: Duration = Duration::from_secs(60);

pub fn mirror_dir(box_dir: &Path) -> PathBuf {
    box_dir.join(MIRROR_DIR)
}

/// Whether `checkout` lives in some box: the registered identity, if any.
pub fn box_of(checkout: &Path) -> Option<super::BoxIdentity> {
    BoxRegistry::global().identity_for_path(checkout)
}

/// The mirror to run transport over for `checkout`, refreshed from the guest
/// (booting the box if it is stopped), or `None` when `checkout` is a host
/// path. `source_repo` is the user's repository the alternates point at.
pub async fn for_transport(checkout: &Path, source_repo: &Path) -> Result<Option<PathBuf>> {
    let Some(identity) = box_of(checkout) else {
        return Ok(None);
    };
    let running = BoxRegistry::global()
        .running_or_start(&identity.agent_id)
        .await?;
    refresh_checkout(&running, checkout, Some(source_repo))
        .await
        .map(Some)
}

/// `<box>/sources.json`: each checkout provisioned into the box and the user's
/// repository it came from, so the stop ladder can refresh every mirror with
/// the right alternates and no database handle.
const SOURCES: &str = "sources.json";

/// Remember that `checkout` in the box was provisioned from `source_repo`.
pub fn record_source(box_dir: &Path, checkout: &Path, source_repo: &Path) -> Result<()> {
    let mut sources = read_sources(box_dir);
    sources.insert(
        checkout.to_string_lossy().into_owned(),
        source_repo.to_string_lossy().into_owned(),
    );
    std::fs::write(box_dir.join(SOURCES), serde_json::to_vec_pretty(&sources)?)?;
    Ok(())
}

fn read_sources(box_dir: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read(box_dir.join(SOURCES))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// [`refresh_running`] for an agent by id: a no-op for one with no running
/// box (another engine's, or a box that is stopped and so already mirrored).
pub async fn refresh_agent(agent_id: &str) -> Result<()> {
    match BoxRegistry::global().channel_for_running(agent_id) {
        Ok(_) => {}
        Err(_) => return Ok(()),
    }
    let running = BoxRegistry::global().running_or_start(agent_id).await?;
    refresh_running(&running).await
}

/// Refresh the mirror from every checkout of a running box: the stop ladder's
/// first rung, so a stopped box still has its objects on the host. A box with
/// no provisioned checkout (a test's) has nothing to mirror.
pub async fn refresh_running(running: &RunningBox) -> Result<()> {
    for (checkout, source) in read_sources(&running.dir) {
        refresh_checkout(running, Path::new(&checkout), Some(Path::new(&source))).await?;
    }
    Ok(())
}

/// `git fetch` the guest checkout's heads, HEAD and tracking refs into the
/// mirror, creating it (with alternates to `source_repo`) on first use.
pub async fn refresh_checkout(
    running: &RunningBox,
    checkout: &Path,
    source_repo: Option<&Path>,
) -> Result<PathBuf> {
    let mirror = mirror_dir(&running.dir);
    ensure_mirror(&mirror, source_repo).await?;
    let mut cmd = crate::git_dist::command(&mirror);
    cmd.env("GIT_SSH_COMMAND", running.channel.git_ssh_command());
    cmd.args([
        "fetch",
        "--quiet",
        "--prune",
        "--no-write-fetch-head",
        &running.git_url(checkout),
        "+refs/heads/*:refs/heads/*",
        "+HEAD:refs/fletch/head",
        "+refs/remotes/origin/*:refs/remotes/origin/*",
    ]);
    let out = output(&mut cmd, "mirror fetch").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "refreshing the box mirror failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(mirror)
}

/// Make `sha` an object in the guest checkout: a commit the host made over
/// the mirror's objects (a signed copy on publish) that the guest must hold
/// before any ref there can name it. A no-op for a host checkout.
pub async fn export_commit(checkout: &Path, sha: &str) -> Result<()> {
    let Some(identity) = box_of(checkout) else {
        return Ok(());
    };
    let running = BoxRegistry::global()
        .running_or_start(&identity.agent_id)
        .await?;
    push(
        &mirror_dir(&running.dir),
        &running.channel,
        &running.git_url(checkout),
        &[format!("+{sha}:refs/fletch/exported")],
    )
    .await
}

/// The mirror's `refs/remotes/origin/*` made the guest checkout's, after a
/// transport fetch or push moved them (what `Transport` does in a host
/// checkout's own `.git`).
pub async fn push_tracking(mirror: &Path, checkout: &Path) -> Result<()> {
    let Some(identity) = box_of(checkout) else {
        return Ok(());
    };
    let running = BoxRegistry::global()
        .running_or_start(&identity.agent_id)
        .await?;
    let refspecs: Vec<String> = TRACKING_SYNC.iter().map(|s| s.to_string()).collect();
    push(
        mirror,
        &running.channel,
        &running.git_url(checkout),
        &refspecs,
    )
    .await
}

/// What [`push_tracking`] pushes. `origin/HEAD` is left out: in the guest it
/// is a symref to `origin/<default>` (provisioning sets it), while the mirror
/// has it at most as a plain ref, fetched from the guest by value and pruned
/// by the next transport fetch. Pruned into the guest, receive-pack deletes
/// through the symref, so `origin/main` went with it: every commit on the base
/// then read as unpublished, and a signed publish refused the base's merge
/// commits as foreign (GitHub commits those as noreply@github.com).
const TRACKING_SYNC: &[&str] = &[
    "--prune",
    "+refs/remotes/origin/*:refs/remotes/origin/*",
    "^refs/remotes/origin/HEAD",
    "+refs/pull/*:refs/pull/*",
];

async fn push(mirror: &Path, channel: &Arc<Channel>, url: &str, refspecs: &[String]) -> Result<()> {
    let mut cmd = crate::git_dist::command(mirror);
    cmd.env("GIT_SSH_COMMAND", channel.git_ssh_command());
    cmd.args(["push", "--quiet", url]).args(refspecs);
    let out = output(&mut cmd, "mirror push").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "pushing into the box failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// `git init --bare` once, with `objects/info/alternates` naming the user's
/// repository's store so a fetch moves only the agent's own objects.
async fn ensure_mirror(mirror: &Path, source_repo: Option<&Path>) -> Result<()> {
    if !mirror.join("HEAD").exists() {
        let mut cmd = crate::git_dist::bare_command();
        cmd.args(["init", "-q", "--bare", &mirror.to_string_lossy()]);
        let out = output(&mut cmd, "mirror init").await?;
        if !out.status.success() {
            return Err(Error::Git(format!(
                "creating the box mirror failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }
    if let Some(source) = source_repo {
        let out = git::git_output(
            source,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?;
        if out.status.success() {
            let objects = Path::new(String::from_utf8_lossy(&out.stdout).trim()).join("objects");
            let alternates = mirror.join("objects/info/alternates");
            let line = format!("{}\n", objects.display());
            if std::fs::read_to_string(&alternates).ok().as_deref() != Some(&line) {
                std::fs::create_dir_all(alternates.parent().expect("objects/info"))?;
                std::fs::write(&alternates, line)?;
            }
        }
    }
    Ok(())
}

async fn output(cmd: &mut tokio::process::Command, what: &str) -> Result<std::process::Output> {
    cmd.kill_on_drop(true).stdin(std::process::Stdio::null());
    tokio::time::timeout(TIMEOUT, cmd.output())
        .await
        .map_err(|_| Error::Git(format!("{what} timed out after {}s", TIMEOUT.as_secs())))?
        .map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testing::{bare, commit_file, git, init_repo};

    /// The tracking sync leaves the guest's `origin/HEAD` symref, and the
    /// branch it names, standing — the mirror has no `origin/HEAD` of its own
    /// to send — while it still moves and prunes every branch. A prune of
    /// `origin/HEAD` deletes through the symref, and the base went with it.
    #[test]
    fn the_tracking_sync_keeps_origin_head_and_the_base_it_names() {
        let td = tempfile::tempdir().unwrap();
        let guest = td.path().join("guest");
        init_repo(&guest);
        let base = commit_file(&guest, "a");
        git(&guest, &["update-ref", "refs/remotes/origin/main", &base]);
        git(&guest, &["update-ref", "refs/remotes/origin/gone", &base]);
        git(
            &guest,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        git(&guest, &["checkout", "-q", "-b", "work"]);
        let moved = commit_file(&guest, "b");

        // The mirror after a transport fetch: the remote's branches, `main`
        // moved on, `gone` deleted upstream, and no `origin/HEAD`.
        let mirror = bare(td.path(), "mirror.git");
        git(
            &mirror,
            &[
                "fetch",
                "-q",
                guest.to_str().unwrap(),
                "refs/heads/*:refs/heads/*",
            ],
        );
        git(&mirror, &["update-ref", "refs/remotes/origin/main", &moved]);
        git(
            &mirror,
            &["update-ref", "refs/remotes/origin/feat/x", &base],
        );

        let mut args = vec!["push", "-q", guest.to_str().unwrap()];
        args.extend(TRACKING_SYNC);
        git(&mirror, &args);

        let refs = git(
            &guest,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname) %(symref)",
                "refs/remotes/origin",
            ],
        );
        assert_eq!(
            refs.lines().map(str::trim_end).collect::<Vec<_>>(),
            vec![
                format!("refs/remotes/origin/HEAD {moved} refs/remotes/origin/main"),
                format!("refs/remotes/origin/feat/x {base}"),
                format!("refs/remotes/origin/main {moved}"),
            ],
        );
    }
}
