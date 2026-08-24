//! The dead-instance orphan sweep and the stale-image GC.
//!
//! Every container Fletch launches carries `fletch.host-pid=<pid>` (which app
//! instance owns it) and `fletch.agent-id=<id>` (which agent it runs) — the
//! labels themselves are runtime-neutral and live in
//! [`container::labels`](crate::sandbox::container::labels). If the
//! app dies without cleanup — crash, force-quit, SIGKILL — its containers keep
//! running; the next startup sweeps them by the same pid-liveness rule the
//! nested-root sweeps use (`sandbox/seatbelt.rs`): remove only containers
//! whose owning pid is gone, never a live side-by-side instance's.
//!
//! The agent-id label answers the narrower question archive/discard asks —
//! "is anything still running for *this* agent?" — via
//! [`remove_agent_containers`]. The supervisor's in-memory kill handle is the
//! primary teardown path; that label sweep is the backstop for when there is
//! no in-memory entry left to kill (see `supervisor::disposition`).
//!
//! Images get the same treatment with one rule ([`sweep_stale_images`]): an
//! image Fletch built (attributed by the `fletch.agent` label — see
//! [`image::AGENT_IMAGE_LABEL`]) that is not one of the current expected tags
//! is removed. That covers old-hash tags left by Dockerfile revisions and
//! untagged leftovers from TTL rebuilds. Anything we can't attribute survives.

use std::time::Duration;

use crate::error::{Error, Result};
use crate::sandbox::container::image_gc::{
    current_tags, image_removal_refs, known_repos, parse_images_line, ImageRow, IMAGES_FORMAT,
    RETIRED_REPOS,
};

use super::{cli, engine, image};

/// The labels `docker run` stamps and every sweep here queries. Re-exported so
/// the `cleanup::host_pid_label` / `cleanup::HOST_PID_LABEL` paths this module's
/// callers already use keep resolving from their runtime-neutral home.
pub(super) use crate::sandbox::container::labels::{
    agent_id_filter, host_pid_label, orphaned_ids, HOST_PID_LABEL, INSPECT_FORMAT,
};

/// Listing/inspect are metadata-only; generous next to their usual
/// milliseconds, so tripping one means the daemon is wedged.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// `docker rm -f` also kills a still-running container's process; give the
/// batched removal room without letting a hung daemon pin the sweep thread.
const REMOVE_TIMEOUT: Duration = Duration::from_secs(60);

/// Remove every fletch-labeled container whose owning host instance is dead.
/// Returns the number removed. Callers gate on the probe and run this off
/// the main thread — see `sweep_orphans_at_startup` in `docker/mod.rs`.
pub fn sweep_orphans() -> Result<usize> {
    let list = cli::run_docker(
        &["ps", "-aq", "--filter", &format!("label={HOST_PID_LABEL}")],
        QUERY_TIMEOUT,
    )?;
    if !list.status.success() {
        return Err(Error::Other(format!(
            "docker ps failed: {}",
            String::from_utf8_lossy(&list.stderr).trim(),
        )));
    }
    let ids: Vec<&str> = std::str::from_utf8(&list.stdout)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if ids.is_empty() {
        return Ok(0);
    }

    let mut inspect_args = vec!["inspect", "-f", INSPECT_FORMAT];
    inspect_args.extend(&ids);
    let inspected = cli::run_docker(&inspect_args, QUERY_TIMEOUT)?;
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
        "removing containers of dead fletch instances"
    );
    let mut rm_args = vec!["rm", "-f"];
    rm_args.extend(orphans.iter().map(String::as_str));
    let removed = cli::run_docker(&rm_args, REMOVE_TIMEOUT)?;
    if !removed.status.success() {
        return Err(Error::Other(format!(
            "docker rm failed: {}",
            String::from_utf8_lossy(&removed.stderr).trim(),
        )));
    }
    Ok(orphans.len())
}

/// Remove every container stamped with `fletch.agent-id=<agent_id>`, running
/// or not. Returns the number removed.
///
/// The disposal counterpart to [`sweep_orphans`], and deliberately *without*
/// its pid-liveness check: the caller has decided this specific agent is going
/// away, so every container bearing its id is fair game — including one owned
/// by this very live instance, which is the whole point (the supervisor may no
/// longer hold a kill handle for it). Attribution is still exact: the label is
/// stamped only by our own `docker run`, and it names one agent.
///
/// Matching on the label rather than the container name is required, not a
/// preference — names carry a random nonce (`engine::util::container_name`),
/// so only the launching process ever knew them.
///
/// Callers gate on the probe and run this off the main path (see
/// `supervisor::disposition`), and treat every failure as non-fatal: a
/// `docker rm` can legitimately lose a race against a `--rm` container
/// finishing on its own, and the user's disposal intent must not hinge on the
/// daemon's cooperation.
pub fn remove_agent_containers(agent_id: &str) -> Result<usize> {
    let list = cli::run_docker(
        &["ps", "-aq", "--filter", &agent_id_filter(agent_id)],
        QUERY_TIMEOUT,
    )?;
    if !list.status.success() {
        return Err(Error::Other(format!(
            "docker ps failed: {}",
            String::from_utf8_lossy(&list.stderr).trim(),
        )));
    }
    let ids: Vec<&str> = std::str::from_utf8(&list.stdout)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if ids.is_empty() {
        return Ok(0);
    }

    tracing::info!(
        agent_id,
        count = ids.len(),
        "removing containers of a disposed agent"
    );
    let mut rm_args = vec!["rm", "-f"];
    rm_args.extend(&ids);
    let removed = cli::run_docker(&rm_args, REMOVE_TIMEOUT)?;
    if !removed.status.success() {
        return Err(Error::Other(format!(
            "docker rm failed: {}",
            String::from_utf8_lossy(&removed.stderr).trim(),
        )));
    }
    Ok(ids.len())
}

/// Remove superseded Fletch agent images. One rule: an image carrying the
/// `fletch.agent` label that is not one of the current expected tags (the set
/// of `image_tag(provider)` across all providers) is removed — old-hash tags
/// from Dockerfile revisions and untagged leftovers from TTL rebuilds alike.
/// [`RETIRED_REPOS`] extends "our namespace" backwards over providers Fletch
/// has dropped, so retiring one doesn't strand its images.
///
/// Legacy path: images built before the label existed carry no ownership
/// proof, so they are removed only on an exact [`LEGACY_TAGS`] match — the
/// closed list of tags pre-label Fletch actually shipped. Neither namespace
/// nor tag shape is trusted on its own: a user image in a Fletch repo, even
/// under a hex tag, never matches. This arm becomes dead weight once
/// pre-label installs age out and can then be deleted along with the list.
///
/// Never touched: current tags, the user's `docker_image` override (excluded
/// defensively even though it can't carry the label), any unlabeled image
/// not on the legacy list, and images in use by a container — `docker rmi`
/// runs WITHOUT `-f`, so an in-use image fails removal, which is expected and
/// logged at debug. Returns the number of images actually removed; callers
/// treat all failures as non-fatal.
pub fn sweep_stale_images() -> Result<usize> {
    let current_tags = current_tags();
    let known_repos = known_repos();
    let override_image = engine::image_override();

    let labeled = list_images(&[
        "images",
        "--filter",
        &format!("label={}", image::AGENT_IMAGE_LABEL),
        "--format",
        IMAGES_FORMAT,
    ])?;
    // Legacy pre-label images: list each Fletch-owned repo by name. (A repo
    // argument to `docker images` matches only that exact repo.) Retired repos
    // are listed too — a pre-label image doesn't stop being ours because the
    // provider was dropped, and the arm still removes nothing that isn't an
    // exact [`LEGACY_TAGS`] match.
    let mut legacy = Vec::new();
    for repo in known_repos
        .iter()
        .copied()
        .chain(RETIRED_REPOS.iter().copied())
    {
        legacy.extend(list_images(&["images", repo, "--format", IMAGES_FORMAT])?);
    }

    let refs = image_removal_refs(
        &labeled,
        &legacy,
        &current_tags,
        &known_repos,
        RETIRED_REPOS,
        LEGACY_TAGS,
        override_image.as_deref(),
    );
    if refs.is_empty() {
        return Ok(0);
    }

    tracing::info!(
        count = refs.len(),
        "removing superseded fletch agent images"
    );
    let mut removed = 0;
    for image_ref in &refs {
        // One rmi per image (not batched): a single in-use image must not
        // taint the exit status the others report. No `-f` — an image backing
        // a running container stays, by design. A transport failure is per-image
        // too: it must not strand the candidates behind it.
        let out = match cli::run_docker(&["rmi", image_ref], REMOVE_TIMEOUT) {
            Ok(out) => out,
            Err(e) => {
                tracing::warn!(
                    target: "fletch::docker",
                    image = %image_ref,
                    error = %e,
                    "docker rmi could not run for this image; continuing the pass",
                );
                continue;
            }
        };
        if out.status.success() {
            removed += 1;
        } else {
            tracing::debug!(
                target: "fletch::docker",
                image = %image_ref,
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "stale image not removed (expected when a container still uses it)",
            );
        }
    }
    Ok(removed)
}

/// Run one `docker images` listing and parse its rows.
fn list_images(args: &[&str]) -> Result<Vec<ImageRow>> {
    let out = cli::run_docker(args, QUERY_TIMEOUT)?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "docker images failed: {}",
            String::from_utf8_lossy(&out.stderr).trim(),
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_images_line)
        .collect())
}

/// Every tag pre-label Fletch ever shipped — the exact, closed set of images
/// the legacy GC arm may remove. Each is `tag_for(repo, dockerfile,
/// entrypoint)` recomputed from the embedded constants at the named git
/// commit (the hash input is `sha256(dockerfile ++ entrypoint)[..12]`); the
/// claude entry was additionally confirmed against a real pre-label install.
/// The label era starts with the commit introducing this list, so it never
/// grows — once pre-label installs have aged out, this arm and list can be
/// deleted wholesale.
const LEGACY_TAGS: &[&str] = &[
    "fletch-agent:1ea320e4ab55", // claude, unchanged pre-label era (at 3870598)
    "fletch-agent-codex:fa189de85caf", // codex, #367..3870598
    "fletch-agent-opencode:87523a7118a0", // opencode, #368..3870598
    "fletch-agent-pi:54ab6c418d9c", // pi, #368..3870598
    "fletch-agent-cursor:2d8ee8975d0d", // cursor, #369 before the --version build check (3557367)
    "fletch-agent-cursor:b84044879c26", // cursor, #369 after it (b77d973..3870598)
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::container::image_gc::is_content_addressed_tag;
    use crate::sandbox::container::labels::{agent_id_label, parse_inspect_line};

    #[test]
    fn label_argv_shapes() {
        assert_eq!(
            host_pid_label(),
            format!("fletch.host-pid={}", std::process::id()),
        );
        assert_eq!(agent_id_label("agent-42"), "fletch.agent-id=agent-42");
        // What `remove_agent_containers` hands `docker ps --filter`: the same
        // label expression, prefixed. Container names can't be used here (they
        // carry a launch-time nonce), so this string is the only handle.
        assert_eq!(
            agent_id_filter("agent-42"),
            "label=fletch.agent-id=agent-42",
        );
    }

    #[test]
    fn inspect_line_parsing() {
        assert_eq!(
            parse_inspect_line("abc123 4242"),
            Some(("abc123".into(), Some(4242))),
        );
        // Missing label → `index` printed an empty string → no pid.
        assert_eq!(parse_inspect_line("abc123 "), Some(("abc123".into(), None)));
        assert_eq!(parse_inspect_line("abc123"), Some(("abc123".into(), None)));
        // Garbage pid → no pid, not a parse crash.
        assert_eq!(
            parse_inspect_line("abc123 not-a-pid"),
            Some(("abc123".into(), None)),
        );
        assert_eq!(parse_inspect_line(""), None);
    }

    /// The sweep's core rule: dead pid → remove; live pid → keep; and any
    /// container we can't attribute to a pid is kept (under-reclaim bias).
    #[test]
    fn selects_only_provably_dead_owners() {
        let stdout = "aaa 100\nbbb 200\nccc \nddd bogus\n";
        let orphans = orphaned_ids(stdout, |pid| pid == 100);
        assert_eq!(orphans, vec!["bbb".to_string()]);
    }

    /// The legacy allowlist stays well-formed: every entry names a repo Fletch
    /// owns — currently or historically ([`RETIRED_REPOS`], which the legacy
    /// listing also covers) — under a content-addressed tag shape. (The hash
    /// values themselves are frozen history — recomputed from the embedded
    /// constants at the commits named on each entry.) The list is docker's
    /// alone: podman ships after the label era, so its GC passes no legacy arm.
    #[test]
    fn legacy_tags_are_fletch_shaped() {
        let mut owned = known_repos();
        owned.extend(RETIRED_REPOS.iter().copied());
        for entry in LEGACY_TAGS {
            let (repo, tag) = entry
                .split_once(':')
                .expect("legacy entry must be repo:tag");
            assert!(owned.contains(repo), "unknown legacy repo: {repo}");
            assert!(is_content_addressed_tag(tag), "malformed legacy tag: {tag}");
        }
    }

    /// The legacy arm at its real call site: the shipped [`LEGACY_TAGS`] and the
    /// sets `sweep_stale_images` builds must actually select a pre-label image.
    /// Nothing else exercises this arm — podman passes an empty list.
    #[test]
    fn the_shipped_legacy_list_selects_a_pre_label_image() {
        let (repo, tag) = LEGACY_TAGS[0].split_once(':').unwrap();
        let legacy = vec![ImageRow {
            id: "aaa".into(),
            repo: repo.into(),
            tag: tag.into(),
        }];
        let refs = image_removal_refs(
            &[],
            &legacy,
            &current_tags(),
            &known_repos(),
            RETIRED_REPOS,
            LEGACY_TAGS,
            None,
        );
        assert_eq!(refs, vec![LEGACY_TAGS[0].to_string()]);
    }

    /// Integration: a labeled image under a Fletch repo with a non-current tag
    /// is swept; an unlabeled image outside Fletch's repos survives.
    /// `FLETCH_DOCKER_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Docker; opt in via FLETCH_DOCKER_TESTS=1"]
    fn sweeps_stale_labeled_images_only() {
        if !crate::sandbox::docker::docker_tests_enabled() {
            return;
        }
        let build = |dockerfile: &str, tag: &str| {
            let ctx = tempfile::tempdir().unwrap();
            std::fs::write(ctx.path().join("Dockerfile"), dockerfile).unwrap();
            let out = cli::run_docker(
                &["build", "-t", tag, &ctx.path().to_string_lossy()],
                Duration::from_secs(120),
            )
            .unwrap();
            assert!(
                out.status.success(),
                "docker build failed: {}",
                String::from_utf8_lossy(&out.stderr),
            );
        };
        // A labeled image under Fletch's repo with a tag no provider owns —
        // exactly what a superseded image looks like.
        let stale_tag = "fletch-agent:000000000000";
        build(
            &format!("FROM busybox\nLABEL {}=claude\n", image::AGENT_IMAGE_LABEL),
            stale_tag,
        );
        // An unlabeled image outside Fletch's repos: must survive.
        let bystander = "fletch-gc-test-bystander:keep";
        build("FROM busybox\nENV FLETCH_GC_TEST=1\n", bystander);

        let removed = sweep_stale_images().unwrap();
        assert!(removed >= 1, "the stale labeled image should be swept");

        let exists = |tag: &str| {
            cli::run_docker(&["image", "inspect", tag], Duration::from_secs(10))
                .unwrap()
                .status
                .success()
        };
        assert!(!exists(stale_tag), "stale labeled image must be gone");
        assert!(exists(bystander), "unlabeled non-fletch image must survive");

        let _ = cli::run_docker(&["rmi", "-f", bystander], Duration::from_secs(30));
    }

    /// Integration: a container labeled with a dead pid is swept; one labeled
    /// with our own (live) pid survives.
    /// `FLETCH_DOCKER_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Docker; opt in via FLETCH_DOCKER_TESTS=1"]
    fn sweeps_dead_instance_containers_only() {
        if !crate::sandbox::docker::docker_tests_enabled() {
            return;
        }
        let run = |label: &str, name: &str| {
            let out = cli::run_docker(
                &[
                    "run", "-d", "--label", label, "--name", name, "busybox", "sleep", "60",
                ],
                Duration::from_secs(60),
            )
            .unwrap();
            assert!(
                out.status.success(),
                "docker run failed: {}",
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
            let out = cli::run_docker(
                &["ps", "-aq", "--filter", &format!("name={name}")],
                Duration::from_secs(10),
            )
            .unwrap();
            !String::from_utf8_lossy(&out.stdout).trim().is_empty()
        };
        assert!(!exists(&dead_name), "dead-instance container must be gone");
        assert!(exists(&live_name), "live-instance container must survive");

        let _ = cli::run_docker(&["rm", "-f", &live_name], Duration::from_secs(30));
    }
}
