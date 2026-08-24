//! Building, inspecting and reclaiming the embedded agent images — one image
//! per supported provider (see [`DockerProvider`]). Their *content* (the
//! Dockerfiles, entrypoints and content-addressed tags) is runtime-neutral and
//! lives in [`container::images`](crate::sandbox::container::images); everything
//! here shells out to docker.
//!
//! Content addressing alone would freeze the *packages inside* an image
//! forever, though: every image installs "latest at build time" (npm installs,
//! cursor's installer), so a stable Dockerfile means a user's containerized CLI
//! never updates while the host CLI does.
//! [`IMAGE_MAX_AGE`](crate::sandbox::container::freshness::IMAGE_MAX_AGE) fixes
//! that with a
//! TTL: at resolution, an existing image older than the TTL is served for the
//! current launch and rebuilt under the same tag in the background
//! (stale-while-revalidate — see [`refresh_in_background_if_needed`]). A
//! host/container CLI version mismatch triggers the same background rebuild
//! even inside the TTL window — a user who just updated their host CLI
//! expects container parity — while the TTL remains the backstop for
//! Docker-only users with no host CLI to compare against. Every
//! embedded image also carries [`AGENT_IMAGE_LABEL`] so superseded images (old
//! hashes after a Dockerfile revision, untagged leftovers after a TTL rebuild)
//! can be garbage-collected — see `cleanup::sweep_stale_images`.
//!
//! Users can bypass all of this with the `docker_image` settings key (see
//! [`resolve_image`]): a user-supplied image is used verbatim — never built,
//! never inspected — and must have the launching provider's CLI on PATH and git
//! installed. The override is global (applies to whichever provider launches).

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use crate::error::Result;
use crate::sandbox::container::freshness::{classify_freshness, version_refresh_wanted, Freshness};
use crate::sandbox::container::images::{image_spec, write_build_context, BASE_IMAGE};
use crate::sandbox::container::progress::{self, BuildEvent};

use super::cli;
use super::DockerProvider;

/// The image content this module builds from, re-exported so the
/// `image::image_tag` / `image::AGENT_IMAGE_LABEL` paths [`super::cleanup`]'s GC
/// uses keep resolving. (The expected-tag and known-repo sets it derives from
/// them now live in
/// [`container::image_gc`](crate::sandbox::container::image_gc).)
pub(super) use crate::sandbox::container::images::{image_tag, AGENT_IMAGE_LABEL};

/// Progress sink for image builds: called once per docker output line. Callers
/// pass a tracing forwarder to log build output, or `&|_| {}` to ignore it.
pub type Progress<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// Builds are slow (base image pull + apt + npm) but bounded: past this we
/// assume a wedged daemon or dead network and fail the spawn with a clear
/// error rather than letting it hang indefinitely.
const BUILD_TIMEOUT: Duration = Duration::from_secs(600);

/// Quick metadata lookups (`docker image inspect`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);

/// `docker rmi` on a superseded base: local layer deletion, I/O-bound but not
/// network-bound. Same bound `cleanup`'s removals use.
const REMOVE_TIMEOUT: Duration = Duration::from_secs(60);

/// The image to launch containers from, honoring the `docker_image` settings
/// key: a non-empty override is returned verbatim (no build, no inspect, no
/// TTL, no version check — the user owns that image's lifecycle); otherwise
/// the embedded image is built if missing, refreshed in the background if
/// older than
/// [`IMAGE_MAX_AGE`](crate::sandbox::container::freshness::IMAGE_MAX_AGE) or
/// version-divergent from the host CLI, and
/// its tag returned. Callers read the settings key and probe the host CLI
/// (`host_cli_version` — see `agent::cached_provider_version`) and pass both
/// in — this module stays DB-free and host-probe-free.
pub fn resolve_image(
    provider: DockerProvider,
    override_image: Option<&str>,
    host_cli_version: Option<&str>,
    on_progress: Progress,
) -> Result<String> {
    if let Some(image) = override_image.map(str::trim).filter(|s| !s.is_empty()) {
        // The override is global and applies verbatim to whichever provider
        // launches (it must carry that provider's CLI + git on PATH).
        // TODO(per-provider-override): a future per-provider image setting would
        // key this on `provider`; today one override serves all.
        tracing::info!(
            image,
            ?provider,
            "using user-supplied docker image (docker_image setting)"
        );
        return Ok(image.to_string());
    }
    let tag = image_tag(provider);
    let already_existed = ensure_image(provider, &tag, on_progress)?;
    if already_existed {
        // A just-built image is fresh by construction (it installed today's
        // latest — if the host still differs, a rebuild can't fix that); a
        // pre-existing one may have passed the TTL or drifted from the host
        // CLI. Stale-while-revalidate: this launch still uses the existing
        // tag, the refresh (if any) happens off-thread.
        refresh_in_background_if_needed(provider, &tag, host_cli_version);
    }
    Ok(tag)
}

/// Serializes every image build process-wide — foreground first-builds and
/// background TTL rebuilds alike. Concurrent spawns during a cold start would
/// otherwise race docker into building the same image N times, and a TTL
/// rebuild must never interleave with a foreground build of the same tag.
static BUILD_LOCK: Mutex<()> = Mutex::new(());

/// Make sure `provider`'s image `tag` exists locally, building its embedded
/// Dockerfile under that tag if it doesn't. Returns whether the image already
/// existed (`true`) or was built just now (`false`) — the caller uses that to
/// skip the TTL check on a fresh build.
pub fn ensure_image(provider: DockerProvider, tag: &str, on_progress: Progress) -> Result<bool> {
    let spec = image_spec(provider);
    let already_existed = ensure_image_with(spec.dockerfile, spec.entrypoint, tag, on_progress)?;
    if !already_existed {
        // Post-build version probe, off-thread: warms the image-version cache
        // for the mismatch trigger without delaying (or ever failing) the
        // launch that just waited out the build.
        let tag = tag.to_string();
        std::thread::spawn(move || cache_image_version_post_build(provider, &tag));
    }
    Ok(already_existed)
}

/// [`ensure_image`] with explicit content — split out so the integration
/// test can exercise the build machinery with a tiny Dockerfile instead of
/// the full agent image.
fn ensure_image_with(
    dockerfile: &str,
    entrypoint: &str,
    tag: &str,
    on_progress: Progress,
) -> Result<bool> {
    if image_exists(tag)? {
        return Ok(true);
    }
    let _guard = BUILD_LOCK.lock().unwrap();
    // Re-check under the lock: a concurrent spawn may have just built it.
    if image_exists(tag)? {
        return Ok(true);
    }

    tracing::info!(tag, "building agent docker image");
    // Broadcast the build lifecycle to the UI. `Started`/`Finished`/`Failed`
    // fire only here, where a foreground build actually runs (a cached image
    // returns above without emitting), so the toast appears only for builds
    // the user is actually waiting on. Each output line is forwarded alongside
    // the caller's own sink so the tracing forwarder / test counter keep
    // working unchanged.
    progress::emit(BuildEvent::Started {
        runtime: super::RUNTIME_NAME,
    });
    let forward = |line: &str| {
        on_progress(line);
        progress::emit(BuildEvent::Line {
            runtime: super::RUNTIME_NAME,
            line: line.to_string(),
        });
    };
    let result = run_build(dockerfile, entrypoint, tag, false, &forward);
    match &result {
        Ok(()) => progress::emit(BuildEvent::Finished {
            runtime: super::RUNTIME_NAME,
        }),
        Err(e) => progress::emit(BuildEvent::Failed {
            runtime: super::RUNTIME_NAME,
            error: e.to_string(),
        }),
    }
    result?;
    tracing::info!(tag, "agent docker image built");
    Ok(false)
}

/// Write the build context and run `docker build -t tag`, streaming
/// output to `on_line`. Shared by the foreground first-build
/// ([`ensure_image_with`], `no_cache: false`) and the background refresh
/// rebuild ([`rebuild_image`], `no_cache: true`); callers hold [`BUILD_LOCK`]
/// and own their event/progress policy.
fn run_build(
    dockerfile: &str,
    entrypoint: &str,
    tag: &str,
    no_cache: bool,
    on_line: Progress,
) -> Result<()> {
    let ctx = tempfile::tempdir()?;
    write_build_context(ctx.path(), dockerfile, entrypoint)?;

    // Every build passes `--pull`, which can move [`BASE_IMAGE`] onto a newer
    // digest and orphan the image we were previously building on. Snapshot the
    // id we're starting from so a successful build can attribute — and reclaim
    // — whatever it displaced (see [`reap_superseded_base`]).
    let base_before = base_image_id();

    let args = build_args(tag, ctx.path(), no_cache);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    cli::run_docker_streaming(&args, BUILD_TIMEOUT, on_line)?;

    reap_superseded_base(base_before.as_deref());
    Ok(())
}

/// Base image ids a `--pull` has displaced and that are still awaiting
/// removal. A freshly orphaned base is usually *not* removable at the moment we
/// notice it: the other providers' images were built on it and hold a child
/// reference until they rebuild too. So the id is parked here and retried after
/// every subsequent build — precisely when such a reference is most likely to
/// have just been dropped.
///
/// In-memory by choice: a missed reclaim costs one base image until some later
/// run pulls again and retries, which doesn't justify a storage surface.
static SUPERSEDED_BASES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Cap on [`SUPERSEDED_BASES`]. One pending id per provider image is the
/// realistic worst case; past that, the oldest is dropped rather than retried
/// for the life of the process.
const MAX_SUPERSEDED_BASES: usize = 8;

/// The local image id currently behind [`BASE_IMAGE`], or `None` when it isn't
/// pulled yet (first build on a clean machine) or the daemon can't answer.
fn base_image_id() -> Option<String> {
    let out = cli::run_docker(
        &["image", "inspect", "--format", "{{.Id}}", BASE_IMAGE],
        INSPECT_TIMEOUT,
    )
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// Park `before` for reclamation when the build moved the base off it. Pure
/// over the queue so the dedupe and the [`MAX_SUPERSEDED_BASES`] cap are
/// unit-testable without a daemon. Returns whether anything was queued.
fn queue_superseded(pending: &mut Vec<String>, before: Option<&str>, after: Option<&str>) -> bool {
    // A missing side means we can't prove a displacement happened: no base
    // pulled yet (first build), or an inspect the daemon didn't answer. Both
    // are "do nothing" — the same under-reclaim bias the image GC takes.
    let (Some(before), Some(after)) = (before, after) else {
        return false;
    };
    if before == after || pending.iter().any(|id| id == before) {
        return false;
    }
    if pending.len() == MAX_SUPERSEDED_BASES {
        pending.remove(0);
    }
    pending.push(before.to_string());
    true
}

/// Reclaim the base images our `--pull`s have displaced. `before` is the id
/// [`BASE_IMAGE`] resolved to when the build that just succeeded started.
///
/// This covers the one thing Fletch orphans that its image GC structurally
/// cannot see: a superseded base carries no [`AGENT_IMAGE_LABEL`] (the label
/// lives in our Dockerfile *above* the `FROM`, so only our own image gets it)
/// and sits in no Fletch-owned repo, so neither arm of
/// `cleanup::image_removal_refs` can attribute it — it would sit untagged and
/// unreferenced forever. Here we can attribute it: we recorded the id before
/// the pull and watched it move.
///
/// `docker rmi` runs WITHOUT `-f`, and that is the entire safety story. The
/// daemon refuses while anything still references the image — one of our
/// not-yet-rebuilt provider images, or something of the user's we know nothing
/// about. A refusal is the expected case rather than an error, and simply
/// leaves the id parked for a later retry.
fn reap_superseded_base(before: Option<&str>) {
    let mut pending = SUPERSEDED_BASES.lock().unwrap();
    if queue_superseded(&mut pending, before, base_image_id().as_deref()) {
        tracing::debug!(
            target: "fletch::docker",
            base = BASE_IMAGE,
            superseded = %before.unwrap_or_default(),
            "--pull moved the base image; queued the predecessor for reclamation",
        );
    }
    pending.retain(|id| match image_exists(id) {
        // Already gone — reclaimed by an earlier pass, the user, or another
        // tool. Stop tracking it.
        Ok(false) => false,
        // Daemon hiccup: keep it and retry after the next build rather than
        // silently forgetting an image we know we orphaned.
        Err(_) => true,
        Ok(true) => {
            let removed = cli::run_docker(&["rmi", id], REMOVE_TIMEOUT)
                .map(|out| out.status.success())
                .unwrap_or(false);
            if removed {
                tracing::info!(
                    target: "fletch::docker",
                    image = %id,
                    "removed the base image a --pull superseded",
                );
            } else {
                tracing::debug!(
                    target: "fletch::docker",
                    image = %id,
                    "superseded base still referenced; will retry after the next build",
                );
            }
            !removed
        }
    });
}

/// `docker build` argv for `tag` from context `ctx`. `--pull` on every build,
/// first build and refresh rebuild alike: the images exist to capture "latest
/// at build time", and a months-old locally cached `node:22-slim` would
/// silently defeat that (docker's layer cache keys on the base image it has,
/// not on what the registry currently serves). It adds no new failure mode —
/// every agent build already needs the network for its install step (npm
/// installs / cursor's curl installer), so "registry unreachable" fails the
/// build either way, and the refresh path treats that as non-fatal.
///
/// `--no-cache` on refresh rebuilds only: `--pull` alone re-fetches the base,
/// but the install `RUN` layer is keyed on its instruction text and would be
/// served from cache whenever the base digest hasn't moved — a rebuild that
/// changes nothing, silently defeating both the TTL and the version-mismatch
/// trigger. First builds keep the cache: a brand-new image has nothing stale
/// to bust, and cross-provider base-layer sharing on cold starts is worth
/// keeping.
fn build_args(tag: &str, ctx: &Path, no_cache: bool) -> Vec<String> {
    let mut args: Vec<String> = vec!["build".into(), "--pull".into()];
    if no_cache {
        args.push("--no-cache".into());
    }
    args.extend(["-t".into(), tag.into(), ctx.to_string_lossy().into_owned()]);
    args
}

/// Why a background refresh rebuild was kicked — log attribution, plus the
/// version trigger's loop-guard bookkeeping on success.
enum RefreshReason {
    /// The image's build date passed the shared TTL
    /// ([`IMAGE_MAX_AGE`](crate::sandbox::container::freshness::IMAGE_MAX_AGE)).
    Ttl,
    /// The host CLI's probed version differs from the container image's;
    /// `guard_pair` is the `host@tag` pair recorded (persistently, on rebuild
    /// success) so the same combination is never retried — see
    /// `engine::record_version_refresh`.
    VersionMismatch { guard_pair: String },
}

/// Decide whether an existing image needs a background rebuild — TTL first,
/// then host/container version parity — and kick it if so. Returns
/// immediately either way. Freshness is never a launch concern: inspect
/// failures, unparseable timestamps, missing versions, and rebuild failures
/// all leave the existing image serving launches. Logged, never propagated.
/// The rebuild is silent for the UI (log lines only): the build toast
/// presents a blocking first-run build ("this can take a few minutes"),
/// which is the wrong message for a refresh the user never waits on, and its
/// failure state demands a dismissal the user shouldn't be bothered with.
///
/// The version trigger fires even when the image is TTL-fresh — a user who
/// just updated their host CLI expects container parity. It compares with
/// plain inequality (no semver ordering) and is inert whenever a side is
/// missing: no host CLI installed, or the container probe failed (the TTL
/// still covers those). Cadence for both triggers is once per app run
/// (`DockerEngine::resolve_image_cached` caches resolution).
fn refresh_in_background_if_needed(
    provider: DockerProvider,
    tag: &str,
    host_cli_version: Option<&str>,
) {
    // One inspect serves both triggers: build date for the TTL, image id to
    // key the container-version cache.
    let (image_id, created_raw) = match cli::run_docker(
        &["image", "inspect", "--format", "{{.Id}} {{.Created}}", tag],
        INSPECT_TIMEOUT,
    ) {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut parts = text.split_whitespace().map(str::to_string);
            match (parts.next(), parts.next()) {
                (Some(id), Some(created)) => (id, created),
                // Malformed inspect output: same treatment as a failed
                // inspect below.
                _ => return,
            }
        }
        // The image resolved a moment ago; a metadata miss now is not worth
        // failing a launch or rebuilding over. Next app run re-checks.
        Ok(_) | Err(_) => return,
    };

    match classify_freshness(&created_raw, chrono::Utc::now()) {
        Freshness::Stale => {
            tracing::info!(
                target: "fletch::docker",
                tag,
                created = %created_raw,
                "agent image is older than IMAGE_MAX_AGE; rebuilding in the background",
            );
            spawn_refresh_rebuild(provider, tag.to_string(), RefreshReason::Ttl);
            return;
        }
        Freshness::Unknown => {
            // Once per app run in practice: resolution is cached per run
            // (`DockerEngine::resolve_image_cached`), so this can't spam.
            tracing::warn!(
                target: "fletch::docker",
                tag,
                created = %created_raw,
                "unparseable image build date; treating the image as fresh",
            );
        }
        Freshness::Fresh => {}
    }

    // TTL-fresh: check version parity. Ordered so the docker-run probe only
    // happens when there's a host version to compare and the pair hasn't
    // already been tried (the pure decision below re-validates everything).
    let Some(host) = host_cli_version else { return };
    let guard_pair = format!("{host}@{tag}");
    if super::engine::version_refresh_attempted(provider.id(), &guard_pair) {
        return;
    }
    let container = image_cli_version(provider, tag, &image_id);
    if !version_refresh_wanted(Some(host), container.as_deref(), false) {
        return;
    }
    tracing::info!(
        target: "fletch::docker",
        tag,
        host,
        container = %container.as_deref().unwrap_or_default(),
        "host CLI version differs from container image; rebuilding in the background",
    );
    spawn_refresh_rebuild(
        provider,
        tag.to_string(),
        RefreshReason::VersionMismatch { guard_pair },
    );
}

/// Kick the background stale-while-revalidate rebuild shared by both refresh
/// triggers: rebuild the same tag, then (on success) record the version
/// trigger's loop guard, re-probe the fresh image's CLI version, and reap the
/// just-untagged predecessor. On failure, warn and keep serving the old image.
fn spawn_refresh_rebuild(provider: DockerProvider, tag: String, reason: RefreshReason) {
    std::thread::spawn(move || match rebuild_image(provider, &tag) {
        Ok(()) => {
            tracing::info!(target: "fletch::docker", tag, "agent image refreshed");
            if let RefreshReason::VersionMismatch { guard_pair } = reason {
                // Recorded on success only: a transient build failure should
                // retry next run, but a *successful* rebuild that still
                // mismatches (host pinned away from latest) must never loop.
                super::engine::record_version_refresh(provider.id(), guard_pair);
            }
            cache_image_version_post_build(provider, &tag);
            // Docker retagged atomically; the predecessor is now untagged.
            // Reap it (and anything else stale) right away.
            match super::cleanup::sweep_stale_images() {
                Ok(0) => {}
                Ok(n) => tracing::info!(
                    target: "fletch::docker",
                    removed = n,
                    "swept superseded agent images after refresh",
                ),
                Err(e) => tracing::debug!(
                    target: "fletch::docker",
                    error = %e,
                    "post-refresh image sweep failed",
                ),
            }
        }
        Err(e) => tracing::warn!(
            target: "fletch::docker",
            tag,
            error = %e,
            "background image refresh failed; keeping the existing image",
        ),
    });
}

/// Rebuild `provider`'s image under the same `tag`, unconditionally (no
/// exists-check — the point is to replace an image that exists). Serialized on
/// [`BUILD_LOCK`] with foreground builds; `--pull --no-cache` (see
/// [`build_args`]) so neither a stale base nor a cached install layer can
/// defeat the refresh. On success docker retags in place and the old image
/// becomes untagged; on failure the old tag is untouched and keeps serving
/// launches.
fn rebuild_image(provider: DockerProvider, tag: &str) -> Result<()> {
    let spec = image_spec(provider);
    let _guard = BUILD_LOCK.lock().unwrap();
    // Build output is free-form (`line` in a field, not the message) so the
    // sentry scrubber drops it — see the privacy invariant in `lib.rs`.
    let on_line = |line: &str| tracing::info!(target: "fletch::docker_build", line = %line, "docker build output");
    run_build(spec.dockerfile, spec.entrypoint, tag, true, &on_line)
}

/// One-shot in-container version probes are a container start + a node CLI's
/// `--version` — seconds normally, and this bound only reaps a wedged daemon.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// The provider CLI's version inside image `tag`, memoized by `image_id` for
/// this app run. In-memory by choice: persistence would buy one skipped
/// `docker run` per provider per app run — not worth a storage surface, and
/// a restart-time re-probe also self-heals if a probe ever cached garbage.
/// A failed probe caches nothing and returns `None` — the version trigger
/// stays inert for that image (the TTL still covers it) and the next app run
/// retries.
fn image_cli_version(provider: DockerProvider, tag: &str, image_id: &str) -> Option<String> {
    static CACHE: std::sync::OnceLock<Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(v) = cache.lock().unwrap().get(image_id) {
        return Some(v.clone());
    }
    let version = probe_image_cli_version(provider, tag)?;
    cache
        .lock()
        .unwrap()
        .insert(image_id.to_string(), version.clone());
    Some(version)
}

/// Run `docker run --rm <tag> <bin> --version` — no mounts, no agent
/// scaffolding; the image's entrypoint just `exec`s the argv — and extract
/// the version with the same parser the host probe uses
/// (`agent::parse_semver`), so the two sides compare like-for-like. The
/// `fletch.host-pid` label is stamped on so that if the CLI probe is killed
/// at timeout while the daemon keeps the container alive, the next startup's
/// orphan sweep can still attribute and reap it.
fn probe_image_cli_version(provider: DockerProvider, tag: &str) -> Option<String> {
    let pid_label = super::cleanup::host_pid_label();
    let out = cli::run_docker(
        &[
            "run",
            "--rm",
            "--label",
            &pid_label,
            tag,
            provider.image_bin(),
            "--version",
        ],
        VERSION_PROBE_TIMEOUT,
    )
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = if !out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stdout)
    } else {
        String::from_utf8_lossy(&out.stderr)
    };
    crate::agent::parse_semver(&text)
}

/// After a successful build (foreground and background): probe the fresh
/// image's CLI version and warm the [`image_cli_version`] cache so the
/// mismatch trigger has a container side to compare. Best-effort — a failed
/// probe logs at debug and the trigger stays inert for this image; it never
/// fails or delays the build that preceded it.
fn cache_image_version_post_build(provider: DockerProvider, tag: &str) {
    let image_id = match cli::run_docker(
        &["image", "inspect", "--format", "{{.Id}}", tag],
        INSPECT_TIMEOUT,
    ) {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Ok(_) | Err(_) => return,
    };
    match image_cli_version(provider, tag, &image_id) {
        Some(version) => tracing::info!(
            target: "fletch::docker",
            tag,
            version,
            "container CLI version probed after build",
        ),
        None => tracing::debug!(
            target: "fletch::docker",
            tag,
            "post-build container CLI version probe failed; version trigger stays inert for this image",
        ),
    }
}

/// Whether `tag` exists locally. A non-zero `image inspect` exit is the
/// documented "no such image" answer (it also covers a down daemon — the
/// subsequent build then fails with docker's own connectivity error, which
/// is the right message for that state).
fn image_exists(tag: &str) -> Result<bool> {
    let out = cli::run_docker(&["image", "inspect", tag], INSPECT_TIMEOUT)?;
    Ok(out.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::container::images::{tag_for, ENTRYPOINT_SH};

    /// The reclamation queue's rules: only a *proven* displacement is queued
    /// (both ids known and different), never twice, and the queue is capped.
    #[test]
    fn superseded_base_queueing() {
        let mut pending = Vec::new();

        // An unmoved base is not a displacement.
        assert!(!queue_superseded(
            &mut pending,
            Some("sha256:a"),
            Some("sha256:a")
        ));
        // Neither is a missing side — no base pulled yet, or an inspect the
        // daemon didn't answer.
        assert!(!queue_superseded(&mut pending, None, Some("sha256:b")));
        assert!(!queue_superseded(&mut pending, Some("sha256:a"), None));
        assert!(pending.is_empty());

        // A genuine move queues the predecessor, exactly once.
        assert!(queue_superseded(
            &mut pending,
            Some("sha256:a"),
            Some("sha256:b")
        ));
        assert!(!queue_superseded(
            &mut pending,
            Some("sha256:a"),
            Some("sha256:c")
        ));
        assert_eq!(pending, vec!["sha256:a".to_string()]);

        // The cap holds, dropping the oldest rather than growing forever.
        for i in 0..MAX_SUPERSEDED_BASES {
            queue_superseded(
                &mut pending,
                Some(&format!("sha256:x{i}")),
                Some("sha256:new"),
            );
        }
        assert_eq!(pending.len(), MAX_SUPERSEDED_BASES);
        assert!(
            !pending.contains(&"sha256:a".to_string()),
            "the oldest entry should have been evicted",
        );
    }

    #[test]
    fn build_argv_shape() {
        // `--pull` on every build, `--no-cache` on refresh rebuilds only: see
        // the `build_args` doc — neither a stale cached base nor a cached
        // install layer may defeat the freshness the rebuilds exist for.
        assert_eq!(
            build_args("fletch-agent:abc123def456", Path::new("/tmp/ctx"), false),
            vec![
                "build",
                "--pull",
                "-t",
                "fletch-agent:abc123def456",
                "/tmp/ctx"
            ],
        );
        assert_eq!(
            build_args("fletch-agent:abc123def456", Path::new("/tmp/ctx"), true),
            vec![
                "build",
                "--pull",
                "--no-cache",
                "-t",
                "fletch-agent:abc123def456",
                "/tmp/ctx"
            ],
        );
    }

    /// The override path must not touch docker at all — it has to work (and
    /// return instantly) on machines where docker isn't even installed.
    #[test]
    fn override_image_skips_build_entirely() {
        let called = std::sync::atomic::AtomicBool::new(false);
        let progress = |_: &str| called.store(true, std::sync::atomic::Ordering::SeqCst);

        // A host version is passed to prove the override path ignores it too:
        // the user's image is never inspected, so there's nothing to compare.
        let image = resolve_image(
            DockerProvider::Claude,
            Some("  ghcr.io/me/custom:1  "),
            Some("v9.9.9"),
            &progress,
        )
        .unwrap();
        assert_eq!(
            image, "ghcr.io/me/custom:1",
            "override is trimmed and used verbatim"
        );
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "override path must never build",
        );
    }

    #[test]
    fn blank_override_falls_through_to_embedded_tag() {
        // Blank means "not set" — but asserting the full resolve would hit
        // docker; assert only the pure decision by checking the tag source.
        assert!(Some(str::trim("   ")).filter(|s| !s.is_empty()).is_none());
        assert!(image_tag(DockerProvider::Claude).starts_with("fletch-agent:"));
    }

    /// Integration: builds a tiny image (busybox base) through the real
    /// machinery, then verifies the second call is a cached no-op.
    /// `FLETCH_DOCKER_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Docker; opt in via FLETCH_DOCKER_TESTS=1"]
    fn builds_tiny_image_and_reuses_it() {
        if !crate::sandbox::docker::docker_tests_enabled() {
            return;
        }
        let dockerfile =
            "FROM busybox\nCOPY entrypoint.sh /entrypoint.sh\nENTRYPOINT [\"/entrypoint.sh\"]\n";
        let tag = tag_for("fletch-agent", dockerfile, ENTRYPOINT_SH);
        // Start clean so the build path actually runs.
        let _ = cli::run_docker(&["rmi", "-f", &tag], Duration::from_secs(30));

        let lines = std::sync::atomic::AtomicUsize::new(0);
        let progress = |_: &str| {
            lines.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        };
        let existed = ensure_image_with(dockerfile, ENTRYPOINT_SH, &tag, &progress).unwrap();
        assert!(!existed, "first call must report a fresh build");
        assert!(
            image_exists(&tag).unwrap(),
            "image should exist after build"
        );
        assert!(
            lines.load(std::sync::atomic::Ordering::SeqCst) > 0,
            "build should have streamed progress lines",
        );

        // Second call: image present, no build, no progress.
        lines.store(0, std::sync::atomic::Ordering::SeqCst);
        let existed = ensure_image_with(dockerfile, ENTRYPOINT_SH, &tag, &progress).unwrap();
        assert!(existed, "second call must report the cached image");
        assert_eq!(
            lines.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "an existing image must not rebuild",
        );

        let _ = cli::run_docker(&["rmi", "-f", &tag], Duration::from_secs(30));
    }

    /// Integration: the in-container version probe runs `<image_bin>
    /// --version` through the image's entrypoint-less argv path and extracts
    /// the version with the host probe's parser — a fake `claude` script in a
    /// busybox image must come back as `v9.9.9`, and the result must be
    /// memoized by image id.
    /// `FLETCH_DOCKER_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Docker; opt in via FLETCH_DOCKER_TESTS=1"]
    fn probes_container_cli_version() {
        if !crate::sandbox::docker::docker_tests_enabled() {
            return;
        }
        let dockerfile = "FROM busybox\nRUN printf '#!/bin/sh\\necho 9.9.9\\n' > /bin/claude && chmod +x /bin/claude\n";
        let tag = tag_for("fletch-agent", dockerfile, "");
        let _ = cli::run_docker(&["rmi", "-f", &tag], Duration::from_secs(30));
        ensure_image_with(dockerfile, "", &tag, &|_| {}).unwrap();

        assert_eq!(
            probe_image_cli_version(DockerProvider::Claude, &tag).as_deref(),
            Some("v9.9.9"),
            "container probe must parse the CLI's --version output",
        );
        // The cached path returns the same answer without another docker run
        // (indirectly observable: it works even against a bogus tag once the
        // id is cached).
        assert_eq!(
            image_cli_version(DockerProvider::Claude, &tag, "test-id-123").as_deref(),
            Some("v9.9.9"),
        );
        assert_eq!(
            image_cli_version(DockerProvider::Claude, "no-such-image:zzz", "test-id-123")
                .as_deref(),
            Some("v9.9.9"),
            "second lookup for the same image id must hit the cache",
        );

        let _ = cli::run_docker(&["rmi", "-f", &tag], Duration::from_secs(30));
    }
}
