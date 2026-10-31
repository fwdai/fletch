//! The host-side bare mirror of a box's checkout: `<box>/mirror.git`,
//! alternates pointing at the user's repository so only
//! the agent's own objects move, refreshed over the channel before anything
//! host-side needs the checkout's objects — publish and transport, which run
//! host git with the user's credentials and so must never run *in* the
//! checkout — and, on demand, the one turn checkpoint a fork carries out of
//! the box (`sandbox::carry_checkpoint`, [`carry_out`]). Reads (`git_state`,
//! diffs) do not come here: they run in the guest through the routed seam.

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

/// Whether `checkout` lives in some box: the registered identity, if any
/// ([`super::box_holding`]).
pub fn box_of(checkout: &Path) -> Option<super::BoxIdentity> {
    super::box_holding(checkout)
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

/// `<box>/disposal.json`: what the fail-closed disposal read
/// (`git_state::check_for_disposal`) found in each checkout of the box when
/// it last stopped, so the auto-archive sweep can judge a sleeping box's
/// checkouts without booting it. Written by the stop ladder after its mirror
/// refresh ([`record_disposal`]) and deleted as the box starts, before any
/// setup wait ([`forget_disposal`]), so it exists only while the box is
/// stopped and was read after everything that ran in it. Goes with the box
/// dir on discard.
const DISPOSAL: &str = "disposal.json";

/// One checkout's entry in [`DISPOSAL`].
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct StoppedDisposal {
    clean_tree: bool,
    unpushed: u32,
    /// When the read was taken, ms since the epoch.
    read_at_ms: i64,
}

/// Read every checkout of `running`, a box whose stop has begun, and write
/// [`DISPOSAL`]. A checkout whose read fails is left out, which the sweep
/// takes as "skip". Best-effort: a failure is logged, and the box stops
/// regardless (with no file, its agent is not archived).
pub(super) async fn record_disposal(running: &RunningBox) {
    let mut checks = std::collections::BTreeMap::new();
    for checkout in read_sources(&running.dir).into_keys() {
        let read = crate::git_state::check_for_disposal(Path::new(&checkout));
        // Passive too, so a read that escapes the scope fails instead of
        // booting the box being stopped.
        let read = super::executor::with_stopping(
            running.agent_id(),
            running.channel.clone(),
            crate::exec::passive(read),
        );
        let read_at_ms = chrono::Utc::now().timestamp_millis();
        match read.await {
            Ok(check) => {
                checks.insert(
                    checkout,
                    StoppedDisposal {
                        clean_tree: check.clean_tree,
                        unpushed: check.unpushed,
                        read_at_ms,
                    },
                );
            }
            Err(e) => tracing::info!(
                agent_id = running.agent_id(),
                checkout = %checkout,
                error = %e,
                "disposal read before stop failed; the checkout will not be auto-archived"
            ),
        }
    }
    let written = serde_json::to_vec_pretty(&checks)
        .map_err(Error::from)
        .and_then(|bytes| std::fs::write(running.dir.join(DISPOSAL), bytes).map_err(Error::from));
    if let Err(e) = written {
        tracing::warn!(agent_id = running.agent_id(), error = %e, "writing the disposal record failed");
    }
}

/// Delete [`DISPOSAL`] as the box in `box_dir` starts (`ensure_running`,
/// before any setup wait): from here on its checkouts can change, so the
/// record no longer describes them.
pub(super) fn forget_disposal(box_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(box_dir.join(DISPOSAL)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// What the stop ladder read in `checkout` when its box last stopped, or
/// `None` when `checkout` is in no box, its box is anything but stopped
/// (preparing to start counts as started: the record goes as it begins), or
/// nothing usable was recorded for it (no record, an unreadable one, a read
/// that failed).
pub fn disposal_at_stop(checkout: &Path) -> Option<crate::git_state::DisposalCheck> {
    let identity = box_of(checkout)?;
    if BoxRegistry::global().state(&identity.agent_id) != super::BoxPhase::Stopped {
        return None;
    }
    let box_dir = identity.box_dir().ok()?;
    let bytes = std::fs::read(box_dir.join(DISPOSAL)).ok()?;
    let mut checks: std::collections::BTreeMap<String, StoppedDisposal> =
        serde_json::from_slice(&bytes).ok()?;
    let check = checks.remove(&*checkout.to_string_lossy())?;
    Some(crate::git_state::DisposalCheck {
        clean_tree: check.clean_tree,
        unpushed: check.unpushed,
    })
}

/// [`refresh_running`] for an agent by id: a no-op for one with no running
/// box (another engine's, or a box that is stopped and so already mirrored).
/// Never boots one.
pub async fn refresh_agent(agent_id: &str) -> Result<()> {
    match BoxRegistry::global().running(agent_id) {
        Some(running) => refresh_running(&running).await,
        None => Ok(()),
    }
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
/// mirror, creating it (with alternates to `source_repo`) on first use. Not
/// its checkpoints: their snapshots carry untracked files, and every stop,
/// turn and transport open runs this; [`carry_out`] fetches the one a fork
/// needs.
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

/// Where [`carry_out`] puts a ref it fetched, one name per call. Outside
/// every namespace [`refresh_checkout`] fetches or prunes.
const CARRY_NS: &str = "refs/fletch-carry";

/// Fetch `refname` (a turn checkpoint) from `checkout`, a checkout in
/// `identity`'s box, into the box's mirror under a name of this call's own,
/// booting the box when it is stopped, and return the mirror and that name:
/// a host repository holding what only the guest has, for a host fetch or a
/// push into another box. The box's checkouts share the one mirror and each
/// pins a turn under the same name, so the original name there could be
/// rewritten from another checkout before the caller reads it; this one
/// cannot. The caller lets it go with [`drop_carried`].
pub async fn carry_out(
    identity: &super::BoxIdentity,
    checkout: &Path,
    refname: &str,
) -> Result<(PathBuf, String)> {
    let running = BoxRegistry::global()
        .running_or_start(&identity.agent_id)
        .await?;
    let source = read_sources(&running.dir)
        .remove(&*checkout.to_string_lossy())
        .map(PathBuf::from);
    let mirror = mirror_dir(&running.dir);
    ensure_mirror(&mirror, source.as_deref()).await?;
    let carried = format!("{CARRY_NS}/{}", uuid::Uuid::new_v4());
    let mut cmd = crate::git_dist::command(&mirror);
    cmd.env("GIT_SSH_COMMAND", running.channel.git_ssh_command());
    cmd.args([
        "fetch",
        "--quiet",
        "--no-tags",
        "--no-write-fetch-head",
        &running.git_url(checkout),
        &format!("+{refname}:{carried}"),
    ]);
    let out = output(&mut cmd, "checkpoint fetch").await?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "fetching the checkpoint out of the box failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok((mirror, carried))
}

/// Delete `carried`, a name [`carry_out`] made, from `mirror`. Best-effort:
/// a leftover pins one snapshot until the box is discarded.
pub async fn drop_carried(mirror: &Path, carried: &str) {
    let mut cmd = crate::git_dist::command(mirror);
    cmd.args(["update-ref", "-d", carried]);
    match output(&mut cmd, "checkpoint ref delete").await {
        Ok(out) if out.status.success() => {}
        Ok(out) => tracing::debug!(
            carried,
            error = %String::from_utf8_lossy(&out.stderr).trim(),
            "dropping a carried checkpoint ref failed"
        ),
        Err(e) => tracing::debug!(carried, error = %e, "dropping a carried checkpoint ref failed"),
    }
}

/// Push `refspecs` from the host repository `from` into `checkout`, a
/// checkout in `identity`'s box, over its channel, booting the box when it is
/// stopped. `from` may be an agent's host checkout, so its config is checked
/// first as for any host git run there.
pub async fn push_into(
    identity: &super::BoxIdentity,
    checkout: &Path,
    from: &Path,
    refspecs: &[String],
) -> Result<()> {
    git::hardening::refuse_steerable_config(from).await?;
    let running = BoxRegistry::global()
        .running_or_start(&identity.agent_id)
        .await?;
    push(from, &running.channel, &running.git_url(checkout), refspecs).await
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

/// `git push` from the host repository `from` into the guest at `url`.
async fn push(from: &Path, channel: &Arc<Channel>, url: &str, refspecs: &[String]) -> Result<()> {
    let mut cmd = crate::git_dist::command(from);
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
