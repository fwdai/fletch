//! Building, inspecting and refreshing the embedded agent images with podman.
//! The image content itself is runtime-neutral —
//! [`container::images`](crate::sandbox::container::images).
//!
//! Every invocation carries the launch's pinned connection (see
//! [`super::machine`]): podman keeps one image store per machine, so an image
//! built or found on one connection says nothing about the one the run uses.
//!
//! Images install "latest at build time", so a content-addressed tag alone would
//! freeze their contents forever. The shared TTL
//! ([`IMAGE_MAX_AGE`](crate::sandbox::container::freshness::IMAGE_MAX_AGE)) and a
//! host/container CLI version mismatch each trigger a background rebuild under
//! the same tag, the current launch still using the existing image.

use std::sync::Mutex;
use std::time::Duration;

use crate::error::Result;
use crate::sandbox::container::freshness::{classify_freshness, version_refresh_wanted, Freshness};
use crate::sandbox::container::images::{image_spec, image_tag, write_build_context};
use crate::sandbox::container::progress::{self, BuildEvent};
use crate::sandbox::container::ContainerProvider;

use super::cli;

/// Progress sink for image builds: called once per podman output line.
pub type Progress<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// Builds are slow (base image pull + apt + npm); past this, assume a wedged
/// connection or dead network and fail the spawn rather than hang.
const BUILD_TIMEOUT: Duration = Duration::from_secs(600);

/// Quick metadata lookups (`podman image inspect`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);

/// In-container `--version` probes: seconds normally; this only reaps a wedged
/// machine.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Serializes every image build process-wide, foreground and background alike,
/// so concurrent spawns can't race podman into building the same tag N times.
/// Separate from docker's lock: the two runtimes keep separate image stores.
static BUILD_LOCK: Mutex<()> = Mutex::new(());

/// The image to launch `provider`'s containers from on `connection`. A non-empty
/// `override_image` (the `podman_image` settings key) is returned verbatim — no
/// build, no inspect, no freshness check — since the user owns that image's
/// lifecycle. Otherwise the embedded image is built if `connection`'s store
/// lacks it, refreshed in the background if stale, and its tag returned.
///
/// The override and `host_cli_version` are passed in rather than read here, so
/// this module stays DB-free and host-probe-free.
pub(super) fn resolve_image(
    provider: ContainerProvider,
    connection: Option<&str>,
    override_image: Option<&str>,
    host_cli_version: Option<&str>,
    on_progress: Progress,
) -> Result<String> {
    if let Some(image) = override_image.map(str::trim).filter(|s| !s.is_empty()) {
        tracing::info!(
            image,
            ?provider,
            "using user-supplied podman image (podman_image setting)"
        );
        return Ok(image.to_string());
    }
    let tag = image_tag(provider);
    let spec = image_spec(provider);
    let already_existed = ensure_image(
        connection,
        &tag,
        spec.dockerfile,
        spec.entrypoint,
        on_progress,
    )?;
    if already_existed {
        // A just-built image is fresh by construction; only a pre-existing one
        // can have passed the TTL or drifted from the host CLI.
        refresh_in_background_if_needed(provider, connection, &tag, host_cli_version);
    } else {
        // Warms the image-version cache for the mismatch trigger without
        // delaying the launch that just waited out the build. The thread owns
        // its connection — it outlives the borrow this call was given.
        let tag = tag.clone();
        let connection = connection.map(str::to_string);
        std::thread::spawn(move || {
            cache_image_version_post_build(provider, connection.as_deref(), &tag)
        });
    }
    Ok(tag)
}

/// Make sure `tag` exists in `connection`'s store, building `dockerfile` under
/// it if it doesn't; returns whether it already existed. The content is a
/// parameter so the integration test can build a tiny Dockerfile instead.
fn ensure_image(
    connection: Option<&str>,
    tag: &str,
    dockerfile: &str,
    entrypoint: &str,
    on_progress: Progress,
) -> Result<bool> {
    if image_exists(connection, tag)? {
        return Ok(true);
    }
    let _guard = BUILD_LOCK.lock().unwrap();
    // Re-check under the lock: a concurrent spawn may have just built it.
    if image_exists(connection, tag)? {
        return Ok(true);
    }

    tracing::info!(tag, "building agent podman image");
    // The UI build lifecycle is emitted only here, past the cached-image
    // returns, so the toast appears only for builds the user waits on.
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
    let result = run_build(connection, dockerfile, entrypoint, tag, false, &forward);
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
    tracing::info!(tag, "agent podman image built");
    Ok(false)
}

/// Write the build context and run `podman build -t tag` on `connection`,
/// streaming output to `on_line`. Callers hold [`BUILD_LOCK`] and own their
/// event/progress policy.
fn run_build(
    connection: Option<&str>,
    dockerfile: &str,
    entrypoint: &str,
    tag: &str,
    no_cache: bool,
    on_line: Progress,
) -> Result<()> {
    // A throwaway context dir, so nothing from the host repo leaks in.
    let ctx = tempfile::tempdir()?;
    write_build_context(ctx.path(), dockerfile, entrypoint)?;
    let args = build_args(tag, ctx.path(), no_cache);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    cli::run_podman_streaming(connection, &args, BUILD_TIMEOUT, on_line)
}

/// `podman build` argv for `tag` from context `ctx`. `--pull` on every build: a
/// months-old cached base would defeat the "latest at build time" the images
/// exist to capture. `--no-cache` on refresh rebuilds only, because the install
/// `RUN` layer is keyed on its instruction text and would otherwise be served
/// from cache whenever the base digest hasn't moved — a rebuild that changes
/// nothing; first builds keep the cache for cross-provider base-layer sharing.
///
/// The connection pin is a global flag, so [`cli::run_podman_streaming`]
/// prepends it rather than it appearing here.
fn build_args(tag: &str, ctx: &std::path::Path, no_cache: bool) -> Vec<String> {
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
    /// The image's build date passed the shared TTL.
    Ttl,
    /// `guard_pair` is the `host@tag` pair recorded on rebuild success, so the
    /// same combination is never retried.
    VersionMismatch { guard_pair: String },
}

/// Kick the whole freshness decision onto a background thread: deciding costs an
/// `image inspect` and possibly a container start for the `--version` probe,
/// neither of which a launch may wait on. The thread owns its data — it outlives
/// the borrows this call was given.
fn refresh_in_background_if_needed(
    provider: ContainerProvider,
    connection: Option<&str>,
    tag: &str,
    host_cli_version: Option<&str>,
) {
    let connection = connection.map(str::to_string);
    let tag = tag.to_string();
    let host_cli_version = host_cli_version.map(str::to_string);
    std::thread::spawn(move || {
        refresh_if_needed(provider, connection, tag, host_cli_version.as_deref())
    });
}

/// Decide whether the image in `connection`'s store needs a rebuild — TTL first,
/// then host/container version parity — and run it if so. Every failure mode
/// leaves the existing image serving launches; logged, never propagated. Silent
/// for the UI: the build toast means a blocking first-run build, not a refresh
/// nobody waits on.
fn refresh_if_needed(
    provider: ContainerProvider,
    connection: Option<String>,
    tag: String,
    host_cli_version: Option<&str>,
) {
    let connection = connection.as_deref();
    let tag = tag.as_str();
    // One inspect serves both triggers: build date for the TTL, image id to key
    // the container-version cache.
    let Some((image_id, created_raw)) = inspect_id_and_created(connection, tag) else {
        // The image resolved a moment ago; a metadata miss now isn't worth
        // rebuilding over. Next app run re-checks.
        return;
    };

    match classify_freshness(&created_raw, chrono::Utc::now()) {
        Freshness::Stale => {
            tracing::info!(
                target: "fletch::podman",
                tag,
                created = %created_raw,
                "agent image is older than IMAGE_MAX_AGE; rebuilding in the background",
            );
            run_refresh_rebuild(provider, connection, tag, RefreshReason::Ttl);
            return;
        }
        Freshness::Unknown => {
            // Can't spam: resolution is cached per app run
            // (`PodmanEngine::resolve_image_cached`).
            tracing::warn!(
                target: "fletch::podman",
                tag,
                created = %created_raw,
                "unparseable image build date; treating the image as fresh",
            );
        }
        Freshness::Fresh => {}
    }

    // TTL-fresh: check version parity. Ordered so the podman-run probe only
    // happens when there's a host version to compare and the pair hasn't
    // already been tried.
    let Some(host) = host_cli_version else { return };
    let guard_pair = format!("{host}@{tag}");
    if super::settings::version_refresh_attempted(provider.id(), &guard_pair) {
        return;
    }
    let container = image_cli_version(provider, connection, tag, &image_id);
    if !version_refresh_wanted(Some(host), container.as_deref(), false) {
        return;
    }
    tracing::info!(
        target: "fletch::podman",
        tag,
        host,
        container = %container.as_deref().unwrap_or_default(),
        "host CLI version differs from container image; rebuilding in the background",
    );
    run_refresh_rebuild(
        provider,
        connection,
        tag,
        RefreshReason::VersionMismatch { guard_pair },
    );
}

/// The stale-while-revalidate rebuild shared by both refresh triggers: rebuild
/// the same tag, then on success record the loop guard, re-probe the CLI
/// version, and sweep the untagged predecessor. On failure, warn and keep
/// serving the old image.
fn run_refresh_rebuild(
    provider: ContainerProvider,
    connection: Option<&str>,
    tag: &str,
    reason: RefreshReason,
) {
    match rebuild_image(provider, connection, tag) {
        Ok(()) => {
            tracing::info!(target: "fletch::podman", tag, "agent image refreshed");
            if let RefreshReason::VersionMismatch { guard_pair } = reason {
                // On success only: a transient build failure should retry next
                // run, but a successful rebuild that still mismatches (host
                // pinned away from latest) must never loop.
                super::settings::record_version_refresh(provider.id(), guard_pair);
            }
            cache_image_version_post_build(provider, connection, tag);
            // Podman retagged in place, so the predecessor is now untagged.
            // This store only — the rebuild touched no other.
            match super::cleanup::sweep_stale_images_on(connection) {
                Ok(0) => {}
                Ok(n) => tracing::info!(
                    target: "fletch::podman",
                    removed = n,
                    "swept superseded agent images after refresh",
                ),
                Err(e) => tracing::debug!(
                    target: "fletch::podman",
                    error = %e,
                    "post-refresh image sweep failed",
                ),
            }
        }
        Err(e) => tracing::warn!(
            target: "fletch::podman",
            tag,
            error = %e,
            "background image refresh failed; keeping the existing image",
        ),
    }
}

/// Rebuild `provider`'s image under the same `tag` in `connection`'s store,
/// unconditionally — the point is to replace an image that exists. On failure
/// the old tag is untouched and keeps serving launches.
fn rebuild_image(provider: ContainerProvider, connection: Option<&str>, tag: &str) -> Result<()> {
    let spec = image_spec(provider);
    let _guard = BUILD_LOCK.lock().unwrap();
    // Free-form output rides in the `line` field, not the message, so the sentry
    // scrubber drops it — see the privacy invariant in `lib.rs`.
    let on_line = |line: &str| tracing::info!(target: "fletch::podman_build", line = %line, "podman build output");
    run_build(
        connection,
        spec.dockerfile,
        spec.entrypoint,
        tag,
        true,
        &on_line,
    )
}

/// The provider CLI's version inside image `tag`, memoized by `image_id` for
/// this app run. The id is a content digest, so the memo is correct across
/// connections: the same id names the same bytes on every machine. A failed
/// probe caches nothing and returns `None`, leaving the version trigger inert
/// for that image until the next app run.
fn image_cli_version(
    provider: ContainerProvider,
    connection: Option<&str>,
    tag: &str,
    image_id: &str,
) -> Option<String> {
    static CACHE: std::sync::OnceLock<Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(v) = cache.lock().unwrap().get(image_id) {
        return Some(v.clone());
    }
    let version = probe_image_cli_version(provider, connection, tag)?;
    cache
        .lock()
        .unwrap()
        .insert(image_id.to_string(), version.clone());
    Some(version)
}

/// Run `podman run --rm <tag> <bin> --version` on `connection`, parsed with the
/// same `agent::parse_semver` the host probe uses so the two sides compare
/// like-for-like. The `fletch.host-pid` label is what lets the next startup's
/// orphan sweep reap this container if the CLI is killed at timeout while podman
/// keeps it alive.
fn probe_image_cli_version(
    provider: ContainerProvider,
    connection: Option<&str>,
    tag: &str,
) -> Option<String> {
    let pid_label = crate::sandbox::container::labels::host_pid_label();
    let out = cli::run_podman_on(
        connection,
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

/// Warm the [`image_cli_version`] cache after a build, so the mismatch trigger
/// has a container side to compare. Best-effort: a failed probe leaves the
/// trigger inert for this image.
fn cache_image_version_post_build(
    provider: ContainerProvider,
    connection: Option<&str>,
    tag: &str,
) {
    let Some((image_id, _)) = inspect_id_and_created(connection, tag) else {
        return;
    };
    match image_cli_version(provider, connection, tag, &image_id) {
        Some(version) => tracing::info!(
            target: "fletch::podman",
            tag,
            version,
            "container CLI version probed after build",
        ),
        None => tracing::debug!(
            target: "fletch::podman",
            tag,
            "post-build container CLI version probe failed; version trigger stays inert for this image",
        ),
    }
}

/// `(image id, creation timestamp)` for `tag` in `connection`'s store, or `None`
/// when podman can't answer or its output doesn't parse.
///
/// Plain JSON rather than a `--format` Go template: a template renders `Created`
/// through `time.Time`'s `String()` (`2026-07-01 12:00:00 +0000 UTC`), while the
/// JSON encoding is the RFC3339 shape [`classify_freshness`] parses.
fn inspect_id_and_created(connection: Option<&str>, tag: &str) -> Option<(String, String)> {
    let out = cli::run_podman_on(connection, &["image", "inspect", tag], INSPECT_TIMEOUT).ok()?;
    if !out.status.success() {
        return None;
    }
    parse_inspect_json(&String::from_utf8_lossy(&out.stdout))
}

/// Pull `Id` and `Created` out of an `image inspect` JSON array. Missing or
/// non-string fields read as "can't answer", which leaves the image alone.
fn parse_inspect_json(stdout: &str) -> Option<(String, String)> {
    let parsed: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let entry = parsed.get(0)?;
    let id = entry.get("Id")?.as_str()?;
    let created = entry.get("Created")?.as_str()?;
    (!id.is_empty() && !created.is_empty()).then(|| (id.to_string(), created.to_string()))
}

/// Whether `tag` exists in `connection`'s store. A non-zero `image inspect` exit
/// is podman's "no such image" answer; it also covers an unreachable machine,
/// where the subsequent build fails with podman's own connectivity error.
fn image_exists(connection: Option<&str>, tag: &str) -> Result<bool> {
    let out = cli::run_podman_on(connection, &["image", "inspect", tag], INSPECT_TIMEOUT)?;
    Ok(out.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::container::images::{tag_for, ENTRYPOINT_SH};

    #[test]
    fn build_argv_shape() {
        // `--pull` always, `--no-cache` on refresh rebuilds only — see the
        // `build_args` doc.
        assert_eq!(
            build_args(
                "fletch-agent:abc123def456",
                std::path::Path::new("/tmp/ctx"),
                false
            ),
            vec![
                "build",
                "--pull",
                "-t",
                "fletch-agent:abc123def456",
                "/tmp/ctx"
            ],
        );
        assert_eq!(
            build_args(
                "fletch-agent:abc123def456",
                std::path::Path::new("/tmp/ctx"),
                true
            ),
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

    #[test]
    fn inspect_json_parsing() {
        let stdout = r#"[
             {
               "Id": "sha256:deadbeef",
               "Created": "2026-07-01T12:00:00.123456789Z",
               "Labels": { "fletch.agent": "claude" }
             }
        ]"#;
        assert_eq!(
            parse_inspect_json(stdout),
            Some((
                "sha256:deadbeef".to_string(),
                "2026-07-01T12:00:00.123456789Z".to_string()
            )),
        );
        let (_, created) = parse_inspect_json(stdout).unwrap();
        assert_ne!(
            classify_freshness(&created, chrono::Utc::now()),
            Freshness::Unknown,
            "podman's JSON timestamp must parse as RFC3339",
        );

        assert_eq!(parse_inspect_json("[]"), None);
        assert_eq!(parse_inspect_json(r#"[{"Id": "sha256:a"}]"#), None);
        assert_eq!(
            parse_inspect_json(r#"[{"Id": "", "Created": "2026-07-01T12:00:00Z"}]"#),
            None,
        );
        assert_eq!(parse_inspect_json("not json"), None);
        assert_eq!(parse_inspect_json(""), None);
    }

    /// The override path must not touch podman at all: it has to work on hosts
    /// where podman isn't installed.
    #[test]
    fn override_image_skips_build_entirely() {
        let called = std::sync::atomic::AtomicBool::new(false);
        let progress = |_: &str| called.store(true, std::sync::atomic::Ordering::SeqCst);

        // The bogus connection and host version prove both are ignored here.
        let image = resolve_image(
            ContainerProvider::Claude,
            Some("no-such-connection"),
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

    /// Integration. `FLETCH_PODMAN_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Podman; opt in via FLETCH_PODMAN_TESTS=1"]
    fn builds_tiny_image_and_reuses_it() {
        if !crate::sandbox::podman::podman_tests_enabled() {
            return;
        }
        let dockerfile =
            "FROM busybox\nCOPY entrypoint.sh /entrypoint.sh\nENTRYPOINT [\"/entrypoint.sh\"]\n";
        let tag = tag_for("fletch-agent", dockerfile, ENTRYPOINT_SH);
        // Start clean so the build path actually runs.
        let _ = cli::run_podman(&["rmi", "-f", &tag], Duration::from_secs(30));

        let lines = std::sync::atomic::AtomicUsize::new(0);
        let progress = |_: &str| {
            lines.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        };
        let existed = ensure_image(None, &tag, dockerfile, ENTRYPOINT_SH, &progress).unwrap();
        assert!(!existed, "first call must report a fresh build");
        assert!(
            image_exists(None, &tag).unwrap(),
            "image should exist after build"
        );
        assert!(
            lines.load(std::sync::atomic::Ordering::SeqCst) > 0,
            "build should have streamed progress lines",
        );

        lines.store(0, std::sync::atomic::Ordering::SeqCst);
        let existed = ensure_image(None, &tag, dockerfile, ENTRYPOINT_SH, &progress).unwrap();
        assert!(existed, "second call must report the cached image");
        assert_eq!(
            lines.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "an existing image must not rebuild",
        );

        let (id, created) = inspect_id_and_created(None, &tag).expect("inspect must answer");
        assert!(!id.is_empty());
        assert_eq!(
            classify_freshness(&created, chrono::Utc::now()),
            Freshness::Fresh,
            "a just-built image must classify fresh (created = {created})",
        );

        let _ = cli::run_podman(&["rmi", "-f", &tag], Duration::from_secs(30));
    }

    /// Integration: a fake `claude` script in a busybox image.
    /// `FLETCH_PODMAN_TESTS=1 cargo test -- --ignored`
    #[test]
    #[ignore = "requires Podman; opt in via FLETCH_PODMAN_TESTS=1"]
    fn probes_container_cli_version() {
        if !crate::sandbox::podman::podman_tests_enabled() {
            return;
        }
        let dockerfile = "FROM busybox\nRUN printf '#!/bin/sh\\necho 9.9.9\\n' > /bin/claude && chmod +x /bin/claude\n";
        let tag = tag_for("fletch-agent", dockerfile, "");
        let _ = cli::run_podman(&["rmi", "-f", &tag], Duration::from_secs(30));
        ensure_image(None, &tag, dockerfile, "", &|_| {}).unwrap();

        assert_eq!(
            probe_image_cli_version(ContainerProvider::Claude, None, &tag).as_deref(),
            Some("v9.9.9"),
            "container probe must parse the CLI's --version output",
        );
        // The cache is observable through the bogus tag below: it answers
        // without a podman run.
        assert_eq!(
            image_cli_version(ContainerProvider::Claude, None, &tag, "test-id-123").as_deref(),
            Some("v9.9.9"),
        );
        assert_eq!(
            image_cli_version(
                ContainerProvider::Claude,
                None,
                "no-such-image:zzz",
                "test-id-123"
            )
            .as_deref(),
            Some("v9.9.9"),
            "second lookup for the same image id must hit the cache",
        );

        let _ = cli::run_podman(&["rmi", "-f", &tag], Duration::from_secs(30));
    }
}
