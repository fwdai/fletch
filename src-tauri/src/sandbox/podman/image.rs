//! Building and inspecting the embedded agent images with podman. Their
//! *content* — the Dockerfiles, entrypoints, and content-addressed tags — is
//! runtime-neutral and lives in
//! [`container::images`](crate::sandbox::container::images), so a Podman agent
//! runs the byte-identical image a Docker agent does (different local store,
//! same recipe, same tag).
//!
//! Deliberately smaller than [`docker::image`](crate::sandbox::docker::image):
//! there is no freshness TTL, no host/container version parity check, no image
//! GC, and no user image override here yet. Missing means build; present means
//! launch.

use std::sync::Mutex;
use std::time::Duration;

use crate::error::Result;
use crate::sandbox::container::images::{image_spec, image_tag, write_build_context};
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

/// Serializes every image build process-wide. Concurrent spawns during a cold
/// start would otherwise race podman into building the same image N times.
/// Separate from docker's lock by design: the two runtimes keep separate image
/// stores, so a docker build is no reason to hold up a podman one.
static BUILD_LOCK: Mutex<()> = Mutex::new(());

/// The image to launch `provider`'s containers from, building it if the store
/// behind `connection` doesn't have it yet. The connection is the launch's
/// pinned one, not the default: each machine keeps its own image store, so an
/// image built or found elsewhere says nothing about the one the run uses.
pub(super) fn resolve_image(
    provider: ContainerProvider,
    connection: Option<&str>,
    on_progress: Progress,
) -> Result<String> {
    let tag = image_tag(provider);
    let spec = image_spec(provider);
    ensure_image(
        connection,
        &tag,
        spec.dockerfile,
        spec.entrypoint,
        on_progress,
    )?;
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
    // Build from a throwaway context dir so nothing from the host repo can
    // leak into the image.
    let ctx = tempfile::tempdir()?;
    write_build_context(ctx.path(), dockerfile, entrypoint)?;
    // `--pull`, like docker's builds: the images exist to capture "latest at
    // build time", and a months-old locally cached base would silently defeat
    // that. It adds no new failure mode — every agent build already needs the
    // network for its install step.
    let ctx_path = ctx.path().to_string_lossy().into_owned();
    cli::run_podman_streaming(
        connection,
        &["build", "--pull", "-t", tag, &ctx_path],
        BUILD_TIMEOUT,
        on_progress,
    )?;
    tracing::info!(tag, "agent podman image built");
    Ok(false)
}

/// Whether `tag` exists in `connection`'s store. A non-zero `image inspect`
/// exit is podman's "no such image" answer (it also covers an unreachable
/// machine — the subsequent build then fails with podman's own connectivity
/// error, which is the right message for that state).
fn image_exists(connection: Option<&str>, tag: &str) -> Result<bool> {
    let out = cli::run_podman_on(connection, &["image", "inspect", tag], INSPECT_TIMEOUT)?;
    Ok(out.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::container::images::{tag_for, ENTRYPOINT_SH};

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

        let _ = cli::run_podman(&["rmi", "-f", &tag], Duration::from_secs(30));
    }
}
