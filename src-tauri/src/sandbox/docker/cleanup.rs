//! Container labels, the dead-instance orphan sweep, and the stale-image GC.
//!
//! Every container Fletch launches carries `fletch.host-pid=<pid>` (which app
//! instance owns it) and `fletch.agent-id=<id>` (which agent it runs). If the
//! app dies without cleanup — crash, force-quit, SIGKILL — its containers keep
//! running; the next startup sweeps them by the same pid-liveness rule the
//! nested-root sweeps use (`sandbox/seatbelt.rs`): remove only containers
//! whose owning pid is gone, never a live side-by-side instance's.
//!
//! Images get the same treatment with one rule ([`sweep_stale_images`]): an
//! image Fletch built (attributed by the `fletch.agent` label — see
//! [`image::AGENT_IMAGE_LABEL`]) that is not one of the current expected tags
//! is removed. That covers old-hash tags left by Dockerfile revisions and
//! untagged leftovers from TTL rebuilds. Anything we can't attribute survives.

use std::collections::HashSet;
use std::time::Duration;

use crate::error::{Error, Result};

use super::{cli, engine, image, DockerProvider};

/// Label carrying the owning Fletch instance's pid.
pub const HOST_PID_LABEL: &str = "fletch.host-pid";

/// Label carrying the agent id a container runs (attribution/debugging; the
/// sweep keys on [`HOST_PID_LABEL`] alone).
pub const AGENT_ID_LABEL: &str = "fletch.agent-id";

/// `fletch.host-pid=<our pid>` — the `--label` value stamped on `docker run`.
pub fn host_pid_label() -> String {
    format!("{HOST_PID_LABEL}={}", std::process::id())
}

/// `fletch.agent-id=<agent_id>` — sibling of [`host_pid_label`].
pub fn agent_id_label(agent_id: &str) -> String {
    format!("{AGENT_ID_LABEL}={agent_id}")
}

/// Listing/inspect are metadata-only; generous next to their usual
/// milliseconds, so tripping one means the daemon is wedged.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// `docker rm -f` also kills a still-running container's process; give the
/// batched removal room without letting a hung daemon pin the sweep thread.
const REMOVE_TIMEOUT: Duration = Duration::from_secs(60);

/// `docker inspect` line format pairing each container id with its owning
/// pid. `index` (rather than a direct field access) yields an empty string
/// when the label is somehow absent, which parses to "no pid" and is skipped
/// — under-reclaiming, never removing something we can't attribute.
const INSPECT_FORMAT: &str = r#"{{.Id}} {{index .Config.Labels "fletch.host-pid"}}"#;

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

/// Parse [`INSPECT_FORMAT`] output and keep the ids whose owning pid is
/// provably dead. A missing or unparsable pid means we can't attribute the
/// container, so it is left alone (same under-reclaim bias as
/// `cleanup_nested_state_roots_in`). Pure — the liveness probe is injected
/// for unit tests.
fn orphaned_ids(inspect_stdout: &str, alive: impl Fn(i32) -> bool) -> Vec<String> {
    inspect_stdout
        .lines()
        .filter_map(parse_inspect_line)
        .filter(|(_, pid)| pid.is_some_and(|p| !alive(p)))
        .map(|(id, _)| id)
        .collect()
}

/// One [`INSPECT_FORMAT`] line → `(container_id, owning_pid)`. The pid is
/// `None` when the label was empty or not a number.
fn parse_inspect_line(line: &str) -> Option<(String, Option<i32>)> {
    let mut parts = line.split_whitespace();
    let id = parts.next()?;
    let pid = parts.next().and_then(|p| p.parse::<i32>().ok());
    Some((id.to_string(), pid))
}

/// `docker images` line format for the image GC listings. Untagged (dangling)
/// images print `<none>` for repository and tag.
const IMAGES_FORMAT: &str = "{{.ID}} {{.Repository}} {{.Tag}}";

/// One `docker images` row: a (repo:tag, image id) pair — the same image id
/// appears in multiple rows when it carries multiple tags, which is exactly
/// what the GC wants: it untags Fletch's name and leaves any other name alone.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageRow {
    id: String,
    repo: String,
    tag: String,
}

impl ImageRow {
    /// Whether this row is a dangling image (`<none>:<none>`) — removable only
    /// by id.
    fn untagged(&self) -> bool {
        self.repo == "<none>" || self.tag == "<none>"
    }

    /// The `repo:tag` name of a tagged row.
    fn named(&self) -> String {
        format!("{}:{}", self.repo, self.tag)
    }

    /// What to hand `docker rmi`: the name for tagged rows (untag just ours),
    /// the id for dangling ones (the only handle they have).
    fn removal_ref(&self) -> String {
        if self.untagged() {
            self.id.clone()
        } else {
            self.named()
        }
    }
}

/// One [`IMAGES_FORMAT`] line → an [`ImageRow`]; `None` on malformed lines
/// (skipped — under-reclaim bias, same as the container sweep).
fn parse_images_line(line: &str) -> Option<ImageRow> {
    let mut parts = line.split_whitespace();
    let row = ImageRow {
        id: parts.next()?.to_string(),
        repo: parts.next()?.to_string(),
        tag: parts.next()?.to_string(),
    };
    Some(row)
}

/// Remove superseded Fletch agent images. One rule: an image carrying the
/// `fletch.agent` label that is not one of the current expected tags (the set
/// of `image_tag(provider)` across all providers) is removed — old-hash tags
/// from Dockerfile revisions and untagged leftovers from TTL rebuilds alike.
///
/// Legacy path: images built before the label existed can only be attributed
/// by name — a non-current tag under one of Fletch's own repos (`fletch-agent`,
/// `fletch-agent-codex`, …; Fletch-owned by construction) is removed too. This
/// arm becomes dead weight once pre-label installs age out and can then be
/// deleted.
///
/// Never touched: current tags, the user's `docker_image` override (excluded
/// defensively even though it can't carry the label), any unlabeled image
/// outside Fletch's repos, and images in use by a container — `docker rmi`
/// runs WITHOUT `-f`, so an in-use image fails removal, which is expected and
/// logged at debug. Returns the number of images actually removed; callers
/// treat all failures as non-fatal.
pub fn sweep_stale_images() -> Result<usize> {
    let current_tags: HashSet<String> =
        DockerProvider::ALL.iter().map(|p| image::image_tag(*p)).collect();
    let known_repos: HashSet<&'static str> =
        DockerProvider::ALL.iter().map(|p| image::image_repo(*p)).collect();
    let override_image = engine::image_override();

    let labeled = list_images(&[
        "images",
        "--filter",
        &format!("label={}", image::AGENT_IMAGE_LABEL),
        "--format",
        IMAGES_FORMAT,
    ])?;
    // Legacy pre-label images: list each Fletch-owned repo by name. (A repo
    // argument to `docker images` matches only that exact repo.)
    let mut legacy = Vec::new();
    for repo in &known_repos {
        legacy.extend(list_images(&["images", repo, "--format", IMAGES_FORMAT])?);
    }

    let refs = image_removal_refs(
        &labeled,
        &legacy,
        &current_tags,
        &known_repos,
        override_image.as_deref(),
    );
    if refs.is_empty() {
        return Ok(0);
    }

    tracing::info!(count = refs.len(), "removing superseded fletch agent images");
    let mut removed = 0;
    for image_ref in &refs {
        // One rmi per image (not batched): a single in-use image must not
        // taint the exit status the others report. No `-f` — an image backing
        // a running container stays, by design.
        let out = cli::run_docker(&["rmi", image_ref], REMOVE_TIMEOUT)?;
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

/// The GC's selection rule, pure (inputs are pre-fetched listings, fixed in
/// tests). `labeled` holds every image carrying the `fletch.agent` label;
/// `legacy` holds the tagged contents of Fletch's own repos (pre-label
/// installs). Returns deduplicated `docker rmi` refs.
///
/// The label is the authority, with one belt-and-braces exception each way:
/// a *labeled* image tagged outside `known_repos` is kept (a user re-tagged
/// our image under their own name — their tag, their call), and an *unlabeled*
/// image inside `known_repos` is removed (the legacy path — the repo names are
/// Fletch-owned by construction). Current tags and the `docker_image` override
/// are always kept.
///
/// Within Fletch's repos, only tags Fletch itself could have written are
/// removal candidates: the content-addressed shape (12 lowercase hex chars —
/// see `image::tag_for`) or no tag at all (a dangling rebuild predecessor).
/// A human-shaped tag like `fletch-agent:backup` — whether the user tagged
/// our labeled image or built their own into our namespace — is theirs to
/// keep: a tag a human wrote is the human's call.
fn image_removal_refs(
    labeled: &[ImageRow],
    legacy: &[ImageRow],
    current_tags: &HashSet<String>,
    known_repos: &HashSet<&'static str>,
    override_image: Option<&str>,
) -> Vec<String> {
    let override_image = override_image.map(str::trim).filter(|s| !s.is_empty());
    // A row that must survive no matter which listing produced it. The
    // override comparison is defensive by design: match its exact `repo:tag`,
    // its bare-repo spelling (docker reads `foo` as `foo:latest`), or the
    // image id (all listings print the same short id).
    let protected = |row: &ImageRow| {
        if !row.untagged() && current_tags.contains(&row.named()) {
            return true;
        }
        let Some(ov) = override_image else { return false };
        ov == row.id
            || (!row.untagged() && (ov == row.named() || (row.tag == "latest" && ov == row.repo)))
    };

    let mut seen = HashSet::new();
    let mut refs = Vec::new();
    let mut push = |row: &ImageRow| {
        let r = row.removal_ref();
        if seen.insert(r.clone()) {
            refs.push(r);
        }
    };

    for row in labeled {
        if !row.untagged() && !known_repos.contains(row.repo.as_str()) {
            continue; // labeled but re-tagged under a user name: never touch
        }
        if !row.untagged() && !is_content_addressed_tag(&row.tag) {
            continue; // human-written tag in our repo: their tag, their call
        }
        if protected(row) {
            continue;
        }
        push(row);
    }
    for row in legacy {
        // The repo filter is load-bearing for the "unlabeled non-fletch →
        // keep" property even though the listing is already repo-scoped:
        // selection must be safe regardless of what the listing fed it.
        if row.untagged() || !known_repos.contains(row.repo.as_str()) {
            continue;
        }
        if !is_content_addressed_tag(&row.tag) {
            continue; // user image built into our namespace: never touch
        }
        if protected(row) {
            continue;
        }
        push(row);
    }
    refs
}

/// Whether a tag has the shape Fletch's content addressing writes — exactly
/// 12 lowercase hex chars (`image::tag_for`'s `sha256[..12]`). Anything else
/// in a Fletch repo was written by a human and is never a removal candidate.
fn is_content_addressed_tag(tag: &str) -> bool {
    tag.len() == 12 && tag.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_argv_shapes() {
        assert_eq!(
            host_pid_label(),
            format!("fletch.host-pid={}", std::process::id()),
        );
        assert_eq!(agent_id_label("agent-42"), "fletch.agent-id=agent-42");
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

    fn row(id: &str, repo: &str, tag: &str) -> ImageRow {
        ImageRow {
            id: id.into(),
            repo: repo.into(),
            tag: tag.into(),
        }
    }

    #[test]
    fn images_line_parsing() {
        assert_eq!(
            parse_images_line("abc123def456 fletch-agent 0011aabbccdd"),
            Some(row("abc123def456", "fletch-agent", "0011aabbccdd")),
        );
        // Dangling images print `<none>` placeholders.
        assert_eq!(
            parse_images_line("abc123def456 <none> <none>"),
            Some(row("abc123def456", "<none>", "<none>")),
        );
        assert_eq!(parse_images_line("abc123def456 fletch-agent"), None);
        assert_eq!(parse_images_line(""), None);
    }

    /// The GC's one rule plus its fences, on fixed listings: labeled + stale
    /// → remove; labeled + current → keep; labeled outside Fletch's repos
    /// (user re-tag) → keep; unlabeled non-fletch → keep; legacy fletch-repo
    /// stale → remove; the `docker_image` override → keep in every spelling.
    #[test]
    fn image_gc_selection() {
        let current_tags: HashSet<String> = ["fletch-agent:cafe00000000".to_string()].into();
        let known_repos: HashSet<&'static str> = ["fletch-agent", "fletch-agent-codex"].into();

        let labeled = vec![
            // Old hash under a Fletch repo: superseded by a Dockerfile revision.
            row("aaa", "fletch-agent", "0dab1e000000"),
            // The current tag: what launches use today.
            row("bbb", "fletch-agent", "cafe00000000"),
            // Untagged leftover of a TTL rebuild: removable only by id.
            row("ccc", "<none>", "<none>"),
            // Labeled but re-tagged under a user's name: their tag, kept.
            row("ddd", "mybackup", "keep"),
            // The override, hypothetically labeled (shouldn't happen): kept.
            row("eee", "ghcr.io/me/custom", "1"),
            // Labeled but human-tagged inside our repo (`docker tag` of our
            // image): their tag, kept.
            row("hhh", "fletch-agent", "backup"),
        ];
        let legacy = vec![
            // Pre-label image in a Fletch-owned repo: removable by name.
            row("fff", "fletch-agent-codex", "badc0debadc0"),
            // Current tag also shows up in the repo-scoped listing: kept.
            row("bbb", "fletch-agent", "cafe00000000"),
            // Selection must be safe even if a listing misbehaves: an
            // unlabeled non-fletch row is never removed.
            row("ggg", "someones-image", "latest"),
            // A user's own image built into our namespace: human tag, kept.
            row("iii", "fletch-agent", "backup"),
        ];

        let refs = image_removal_refs(
            &labeled,
            &legacy,
            &current_tags,
            &known_repos,
            Some("ghcr.io/me/custom:1"),
        );
        assert_eq!(
            refs,
            vec![
                "fletch-agent:0dab1e000000".to_string(),
                "ccc".to_string(),
                "fletch-agent-codex:badc0debadc0".to_string(),
            ],
        );
    }

    /// Only tags Fletch's content addressing could have written count as
    /// removal candidates — exactly 12 lowercase hex chars.
    #[test]
    fn content_addressed_tag_shape() {
        assert!(is_content_addressed_tag("0123abcdef01"));
        assert!(is_content_addressed_tag("000000000000"));
        // Human-shaped tags, wrong length, uppercase: all kept.
        assert!(!is_content_addressed_tag("backup"));
        assert!(!is_content_addressed_tag("latest"));
        assert!(!is_content_addressed_tag("0123ABCDEF01"));
        assert!(!is_content_addressed_tag("0123abcdef0"));
        assert!(!is_content_addressed_tag("0123abcdef012"));
        assert!(!is_content_addressed_tag(""));
    }

    /// Override matching is defensive across spellings: exact `repo:tag`,
    /// bare repo (docker's implicit `:latest`), and image id all protect.
    #[test]
    fn image_gc_override_spellings() {
        let current_tags = HashSet::new();
        let known_repos: HashSet<&'static str> = ["fletch-agent"].into();
        // Hypothetical worst case: the user's override lives *inside* a
        // Fletch repo under a content-addressed-looking tag (anything
        // human-shaped is already kept by the tag-shape guard).
        let labeled = vec![
            row("aaa", "fletch-agent", "aaaaaaaaaaaa"),
            row("bbb", "fletch-agent", "bbbbbbbbbbbb"),
        ];

        // Exact repo:tag override protects that row only.
        let refs = image_removal_refs(
            &labeled,
            &[],
            &current_tags,
            &known_repos,
            Some("fletch-agent:aaaaaaaaaaaa"),
        );
        assert_eq!(refs, vec!["fletch-agent:bbbbbbbbbbbb".to_string()]);

        // Id override protects by id.
        let refs = image_removal_refs(&labeled, &[], &current_tags, &known_repos, Some("bbb"));
        assert_eq!(refs, vec!["fletch-agent:aaaaaaaaaaaa".to_string()]);

        // A bare-repo override reads as `:latest` — and a `:latest` row in our
        // repo is kept by the tag-shape guard even before the override check,
        // with or without the override present.
        let with_latest = vec![row("lll", "fletch-agent", "latest")];
        for ov in [Some("fletch-agent"), None] {
            assert!(image_removal_refs(&with_latest, &[], &current_tags, &known_repos, ov)
                .is_empty());
        }

        // Blank override protects nothing (same as None).
        let refs = image_removal_refs(&labeled, &[], &current_tags, &known_repos, Some("  "));
        assert_eq!(refs.len(), 2);

        // Overlapping listings dedupe to one removal ref.
        let refs = image_removal_refs(&labeled, &labeled, &current_tags, &known_repos, None);
        assert_eq!(refs.len(), 2);
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
