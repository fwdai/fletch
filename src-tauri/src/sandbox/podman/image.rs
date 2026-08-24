//! Building, inspecting and refreshing the embedded agent images with podman.
//! Their *content* — the Dockerfiles, entrypoints, and content-addressed tags —
//! is runtime-neutral and lives in
//! [`container::images`](crate::sandbox::container::images), so a Podman agent
//! runs the byte-identical image a Docker agent does (different local store,
//! same recipe, same tag). Everything here shells out to podman.
//!
//! Every invocation carries the launch's pinned connection (see
//! [`super::machine`]). That is not a detail: podman keeps one image store per
//! machine, so an image built or found on one connection says nothing about the
//! one the run will use — an unpinned inspect could report "present" for a store
//! the container never touches, and an unpinned build would land the image on the
//! wrong machine.
//!
//! Content addressing alone would freeze the *packages inside* an image
//! forever: every image installs "latest at build time" (npm installs, cursor's
//! installer), so a stable Dockerfile means a user's containerized CLI never
//! updates while the host CLI does. The shared TTL
//! ([`IMAGE_MAX_AGE`](crate::sandbox::container::freshness::IMAGE_MAX_AGE))
//! fixes that: at resolution, an existing image older than the TTL is served for
//! the current launch and rebuilt under the same tag in the background
//! (stale-while-revalidate — see [`refresh_in_background_if_needed`]). A
//! host/container CLI version mismatch triggers the same background rebuild even
//! inside the TTL window — a user who just updated their host CLI expects
//! container parity — while the TTL remains the backstop for Podman-only users
//! with no host CLI to compare against.
//!
//! Users can bypass all of this with the `podman_image` settings key (see
//! [`resolve_image`]): a user-supplied image is used verbatim — never built,
//! never inspected — and must have the launching provider's CLI on PATH and git
//! installed.
//!
//! What Podman's freshness path does *not* mirror is docker's
//! `reap_superseded_base`: reclaiming a base image a `--pull` displaced needs a
//! before/after id snapshot around every build plus a retry queue, and podman's
//! rootless store makes a stranded base cheaper to leave than to chase. The
//! label-driven GC in [`super::cleanup`] covers everything Fletch's own builds
//! tag.

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

/// Builds are slow (base image pull + apt + npm) but bounded: past this we
/// assume a wedged machine connection or dead network and fail the spawn with a
/// clear error rather than letting it hang indefinitely.
const BUILD_TIMEOUT: Duration = Duration::from_secs(600);

/// Quick metadata lookups (`podman image inspect`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);

/// One-shot in-container version probes are a container start + a node CLI's
/// `--version` — seconds normally, and this bound only reaps a wedged machine.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Serializes every image build process-wide — foreground first-builds and
/// background refresh rebuilds alike. Concurrent spawns during a cold start
/// would otherwise race podman into building the same image N times, and a
/// refresh rebuild must never interleave with a foreground build of the same
/// tag. Separate from docker's lock by design: the two runtimes keep separate
/// image stores, so a docker build is no reason to hold up a podman one.
static BUILD_LOCK: Mutex<()> = Mutex::new(());

/// The image to launch `provider`'s containers from on `connection`, honoring
/// the `podman_image` settings key: a non-empty override is returned verbatim
/// (no build, no inspect, no TTL, no version check — the user owns that image's
/// lifecycle); otherwise the embedded image is built if the store behind
/// `connection` doesn't have it, refreshed in the background if stale or
/// version-divergent from the host CLI, and its tag returned.
///
/// `connection` is the launch's pinned one, not the default: each machine keeps
/// its own image store, so an image built or found elsewhere says nothing about
/// the one the run uses.
///
/// Callers read the settings key and probe the host CLI (`host_cli_version` —
/// see `agent::cached_provider_version`) and pass both in, so this module stays
/// DB-free and host-probe-free.
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
        // A just-built image is fresh by construction (it installed today's
        // latest — if the host still differs, a rebuild can't fix that); a
        // pre-existing one may have passed the TTL or drifted from the host CLI.
        // Stale-while-revalidate: this launch still uses the existing tag, the
        // refresh (if any) happens off-thread.
        refresh_in_background_if_needed(provider, connection, &tag, host_cli_version);
    } else {
        // Post-build version probe, off-thread: warms the image-version cache
        // for the mismatch trigger without delaying (or ever failing) the launch
        // that just waited out the build. The spawned thread owns its connection
        // — it outlives the borrow this call was given.
        let tag = tag.clone();
        let connection = connection.map(str::to_string);
        std::thread::spawn(move || {
            cache_image_version_post_build(provider, connection.as_deref(), &tag)
        });
    }
    Ok(tag)
}

/// Make sure `tag` exists in `connection`'s store, building `dockerfile` under
/// it if it doesn't. Returns whether the image already existed. Takes the
/// content explicitly so the integration test can exercise the build machinery
/// with a tiny Dockerfile instead of the full agent image.
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
    // Broadcast the build lifecycle to the UI. `Started`/`Finished`/`Failed`
    // fire only here, where a foreground build actually runs (a cached image
    // returns above without emitting), so the toast appears only for builds the
    // user is actually waiting on. Each output line is forwarded alongside the
    // caller's own sink so the tracing forwarder / test counter keep working
    // unchanged.
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
/// streaming output to `on_line`. Shared by the foreground first-build
/// ([`ensure_image`], `no_cache: false`) and the background refresh rebuild
/// ([`rebuild_image`], `no_cache: true`); callers hold [`BUILD_LOCK`] and own
/// their event/progress policy.
fn run_build(
    connection: Option<&str>,
    dockerfile: &str,
    entrypoint: &str,
    tag: &str,
    no_cache: bool,
    on_line: Progress,
) -> Result<()> {
    // A throwaway context dir, so nothing from the host repo can leak into the
    // image.
    let ctx = tempfile::tempdir()?;
    write_build_context(ctx.path(), dockerfile, entrypoint)?;
    let args = build_args(tag, ctx.path(), no_cache);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    cli::run_podman_streaming(connection, &args, BUILD_TIMEOUT, on_line)
}

/// `podman build` argv for `tag` from context `ctx`. `--pull` on every build,
/// first build and refresh rebuild alike: the images exist to capture "latest at
/// build time", and a months-old locally cached base would silently defeat that.
/// It adds no new failure mode — every agent build already needs the network for
/// its install step.
///
/// `--no-cache` on refresh rebuilds only: `--pull` alone re-fetches the base, but
/// the install `RUN` layer is keyed on its instruction text and would be served
/// from cache whenever the base digest hasn't moved — a rebuild that changes
/// nothing, silently defeating both the TTL and the version-mismatch trigger.
/// First builds keep the cache: a brand-new image has nothing stale to bust, and
/// cross-provider base-layer sharing on cold starts is worth keeping.
///
/// The connection pin is *not* here: it's a global flag that has to precede the
/// subcommand, so [`cli::run_podman_streaming`] prepends it.
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
    /// The host CLI's probed version differs from the container image's;
    /// `guard_pair` is the `host@tag` pair recorded (persistently, on rebuild
    /// success) so the same combination is never retried.
    VersionMismatch { guard_pair: String },
}

/// Kick the whole freshness decision onto a background thread and return: the
/// decision itself costs an `image inspect` and, past it, a full container start
/// for the in-image `--version` probe, neither of which a launch may wait on.
///
/// The thread owns its data — it outlives the borrows this call was given.
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
/// then host/container version parity — and run it if so. Already off the launch
/// path (see [`refresh_in_background_if_needed`]). Freshness is never a launch
/// concern: inspect failures, unparseable timestamps, missing versions, and
/// rebuild failures all leave the existing image serving launches. Logged, never
/// propagated. The rebuild is silent for the UI (log lines only): the build toast
/// presents a blocking first-run build, which is the wrong message for a refresh
/// the user never waits on.
///
/// Everything below — inspect, probe, rebuild, post-rebuild sweep — rides the
/// connection that triggered it, so a refresh replaces the image in the store the
/// launch actually read from.
///
/// Cadence for both triggers is once per app run: resolution is cached per
/// (provider, override, connection) (`PodmanEngine::resolve_image_cached`), which
/// still holds now that the cache lock is released across the resolve — two
/// simultaneously *cold* launches of the same key could each land here, and the
/// second's rebuild is serialized behind the first on [`BUILD_LOCK`].
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
        // The image resolved a moment ago; a metadata miss now is not worth
        // failing a launch or rebuilding over. Next app run re-checks.
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
            // Once per app run in practice: resolution is cached per run
            // (`PodmanEngine::resolve_image_cached`), so this can't spam.
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
    // happens when there's a host version to compare and the pair hasn't already
    // been tried (the pure decision below re-validates everything).
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
/// the same tag, then (on success) record the version trigger's loop guard,
/// re-probe the fresh image's CLI version, and sweep the just-untagged
/// predecessor. On failure, warn and keep serving the old image.
///
/// Runs on [`refresh_in_background_if_needed`]'s thread, never a launch's, and
/// every call it makes rides the launch's `connection` rather than whatever the
/// default has become in the meantime.
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
                // Recorded on success only: a transient build failure should
                // retry next run, but a *successful* rebuild that still
                // mismatches (host pinned away from latest) must never loop.
                super::settings::record_version_refresh(provider.id(), guard_pair);
            }
            cache_image_version_post_build(provider, connection, tag);
            // Podman retagged in place; the predecessor is now untagged. Reap
            // it (and anything else stale) right away — in this store only,
            // since this is the only one the rebuild touched.
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
/// unconditionally (no exists-check — the point is to replace an image that
/// exists). Serialized on [`BUILD_LOCK`] with foreground builds; `--pull
/// --no-cache` (see [`build_args`]) so neither a stale base nor a cached install
/// layer can defeat the refresh. On success podman retags in place and the old
/// image becomes untagged; on failure the old tag is untouched and keeps serving
/// launches.
fn rebuild_image(provider: ContainerProvider, connection: Option<&str>, tag: &str) -> Result<()> {
    let spec = image_spec(provider);
    let _guard = BUILD_LOCK.lock().unwrap();
    // Build output is free-form (`line` in a field, not the message) so the
    // sentry scrubber drops it — see the privacy invariant in `lib.rs`.
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
/// connections: the same id names the same bytes on every machine. In-memory by
/// choice: persistence would buy one skipped `podman run` per provider per app
/// run — not worth a storage surface, and a restart-time re-probe also self-heals
/// if a probe ever cached garbage. A failed probe caches nothing and returns
/// `None` — the version trigger stays inert for that image (the TTL still covers
/// it) and the next app run retries.
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

/// Run `podman run --rm <tag> <bin> --version` on `connection` — no mounts, no
/// agent scaffolding; the image's entrypoint just `exec`s the argv — and extract
/// the version with the same parser the host probe uses (`agent::parse_semver`),
/// so the two sides compare like-for-like. The `fletch.host-pid` label is stamped
/// on so that if the CLI probe is killed at timeout while podman keeps the
/// container alive, the next startup's orphan sweep can still attribute and reap
/// it — and because that sweep runs per connection, the pin is what puts the
/// container where the sweep will look.
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

/// After a successful build (foreground and background): probe the fresh image's
/// CLI version and warm the [`image_cli_version`] cache so the mismatch trigger
/// has a container side to compare. Best-effort — a failed probe logs at debug
/// and the trigger stays inert for this image; it never fails or delays the build
/// that preceded it.
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
/// Read from `podman image inspect`'s plain JSON rather than a `--format` Go
/// template: podman models `Created` as a `time.Time`, which a template renders
/// through its `String()` method (`2026-07-01 12:00:00 +0000 UTC`) while the JSON
/// encoding is RFC3339 — the shape [`classify_freshness`] parses and docker's
/// template happens to already produce.
fn inspect_id_and_created(connection: Option<&str>, tag: &str) -> Option<(String, String)> {
    let out = cli::run_podman_on(connection, &["image", "inspect", tag], INSPECT_TIMEOUT).ok()?;
    if !out.status.success() {
        return None;
    }
    parse_inspect_json(&String::from_utf8_lossy(&out.stdout))
}

/// Pull `Id` and `Created` out of an `image inspect` JSON array (one entry per
/// inspected image; we always inspect exactly one). Split out of
/// [`inspect_id_and_created`] so the parsing is unit-testable without a machine.
/// Missing or non-string fields read as "can't answer" — the caller's
/// under-reclaim bias then leaves the image alone.
fn parse_inspect_json(stdout: &str) -> Option<(String, String)> {
    let parsed: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let entry = parsed.get(0)?;
    let id = entry.get("Id")?.as_str()?;
    let created = entry.get("Created")?.as_str()?;
    (!id.is_empty() && !created.is_empty()).then(|| (id.to_string(), created.to_string()))
}

/// Whether `tag` exists in `connection`'s store. A non-zero `image inspect` exit
/// is podman's "no such image" answer (it also covers an unreachable machine —
/// the subsequent build then fails with podman's own connectivity error, which is
/// the right message for that state).
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
        // `--pull` on every build, `--no-cache` on refresh rebuilds only: see
        // the `build_args` doc — neither a stale cached base nor a cached
        // install layer may defeat the freshness the rebuilds exist for.
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

    /// The inspect parse, on podman's real output shape: an RFC3339 `Created`
    /// (which the shared TTL classifier accepts) and the short-form `Id`.
    /// Anything malformed reads as "can't answer".
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
        // The timestamp it yields must be the shape the shared classifier reads.
        let (_, created) = parse_inspect_json(stdout).unwrap();
        assert_ne!(
            classify_freshness(&created, chrono::Utc::now()),
            Freshness::Unknown,
            "podman's JSON timestamp must parse as RFC3339",
        );

        // Empty array (image vanished between calls), missing fields, blank
        // values, and non-JSON all read as "no answer".
        assert_eq!(parse_inspect_json("[]"), None);
        assert_eq!(parse_inspect_json(r#"[{"Id": "sha256:a"}]"#), None);
        assert_eq!(
            parse_inspect_json(r#"[{"Id": "", "Created": "2026-07-01T12:00:00Z"}]"#),
            None,
        );
        assert_eq!(parse_inspect_json("not json"), None);
        assert_eq!(parse_inspect_json(""), None);
    }

    /// The override path must not touch podman at all — it has to work (and
    /// return instantly) on machines where podman isn't even installed, and it
    /// takes the connection like every other path without ever using it.
    #[test]
    fn override_image_skips_build_entirely() {
        let called = std::sync::atomic::AtomicBool::new(false);
        let progress = |_: &str| called.store(true, std::sync::atomic::Ordering::SeqCst);

        // A host version is passed to prove the override path ignores it too:
        // the user's image is never inspected, so there's nothing to compare.
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

    /// Integration: builds a tiny image (busybox base) through the real
    /// machinery, then verifies the second call is a cached no-op.
    /// `FLETCH_PODMAN_TESTS=1 cargo test -- --ignored`
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

        // Second call: image present, no build, no progress.
        lines.store(0, std::sync::atomic::Ordering::SeqCst);
        let existed = ensure_image(None, &tag, dockerfile, ENTRYPOINT_SH, &progress).unwrap();
        assert!(existed, "second call must report the cached image");
        assert_eq!(
            lines.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "an existing image must not rebuild",
        );

        // The freshness path's inspect must resolve against a real image: an id
        // and an RFC3339 build date the shared classifier calls fresh.
        let (id, created) = inspect_id_and_created(None, &tag).expect("inspect must answer");
        assert!(!id.is_empty());
        assert_eq!(
            classify_freshness(&created, chrono::Utc::now()),
            Freshness::Fresh,
            "a just-built image must classify fresh (created = {created})",
        );

        let _ = cli::run_podman(&["rmi", "-f", &tag], Duration::from_secs(30));
    }

    /// Integration: the in-container version probe runs `<image_bin> --version`
    /// through the image's argv path and extracts the version with the host
    /// probe's parser — a fake `claude` script in a busybox image must come back
    /// as `v9.9.9`, and the result must be memoized by image id.
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
        // The cached path returns the same answer without another podman run
        // (indirectly observable: it works even against a bogus tag once the id
        // is cached).
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
