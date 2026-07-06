//! The embedded agent image: what containers run, built on demand.
//!
//! The Dockerfile and entrypoint are compiled into the binary and the image
//! tag is derived from their content (`fletch-agent:<sha256[..12]>`), so
//! shipping a change to either automatically produces a new tag — the stale
//! image is simply never referenced again and the next spawn rebuilds. No
//! version bookkeeping, no manual invalidation.
//!
//! Users can bypass all of this with the `docker_image` settings key (see
//! [`resolve_image`]): a user-supplied image is used verbatim — never built,
//! never inspected — and must have `claude` on PATH and git installed.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use crate::error::Result;

use super::cli;
use super::progress::{self, BuildEvent};

/// Progress sink for image builds: called once per docker output line. Callers
/// pass a tracing forwarder to log build output, or `&|_| {}` to ignore it.
pub type Progress<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// The agent container image. Debian-slim keeps apt available for the tools
/// claude needs at runtime (`git`, `rg`, `jq`, `procps` for /proc-based
/// process introspection) while staying small; node 22 is claude-code's
/// supported runtime. The `chmod` guarantees the entrypoint is executable
/// regardless of the mode `COPY` picked up from the build context (context
/// files are written at build time on the host — see [`ensure_image`]).
pub const DOCKERFILE: &str = r#"FROM node:22-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    git curl ca-certificates ripgrep jq procps \
 && rm -rf /var/lib/apt/lists/*
RUN npm install -g @anthropic-ai/claude-code
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
"#;

/// PID-1 shim. The container gets `HOME=<host home>` (a path that does not
/// exist in the image), so the entrypoint creates it and seeds the minimal
/// `~/.claude.json` claude needs to skip interactive onboarding. Seeding is
/// conditional and the file is container-ephemeral by design: bind-mounting the
/// real `~/.claude.json` would break on claude's atomic rename-replace writes.
pub const ENTRYPOINT_SH: &str = r#"#!/bin/sh
set -e
mkdir -p "$HOME"
if [ ! -f "$HOME/.claude.json" ]; then
  printf '{"hasCompletedOnboarding": true}\n' > "$HOME/.claude.json"
fi
exec "$@"
"#;

/// Builds are slow (base image pull + apt + npm) but bounded: past this we
/// assume a wedged daemon or dead network and fail the spawn with a clear
/// error rather than letting it hang indefinitely.
const BUILD_TIMEOUT: Duration = Duration::from_secs(600);

/// Quick metadata lookups (`docker image inspect`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The content-addressed tag for the embedded image.
pub fn image_tag() -> String {
    tag_for(DOCKERFILE, ENTRYPOINT_SH)
}

/// `fletch-agent:<sha256(dockerfile + entrypoint)[..12]>` — 12 hex chars, the
/// same abbreviation depth docker itself uses for short ids.
fn tag_for(dockerfile: &str, entrypoint: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(dockerfile.as_bytes());
    hasher.update(entrypoint.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("fletch-agent:{}", &hex[..12])
}

/// The image to launch containers from, honoring the `docker_image` settings
/// key: a non-empty override is returned verbatim (no build, no inspect —
/// the user owns that image's lifecycle); otherwise the embedded image is
/// built if missing and its tag returned. Callers read the settings key and
/// pass it in — this module stays DB-free.
pub fn resolve_image(override_image: Option<&str>, on_progress: Progress) -> Result<String> {
    if let Some(image) = override_image.map(str::trim).filter(|s| !s.is_empty()) {
        tracing::info!(
            image,
            "using user-supplied docker image (docker_image setting)"
        );
        return Ok(image.to_string());
    }
    let tag = image_tag();
    ensure_image(&tag, on_progress)?;
    Ok(tag)
}

/// Make sure `tag` exists locally, building the embedded Dockerfile under
/// that tag if it doesn't. Builds are serialized process-wide: concurrent
/// spawns during a cold start would otherwise race docker into building the
/// same image N times.
pub fn ensure_image(tag: &str, on_progress: Progress) -> Result<()> {
    ensure_image_with(DOCKERFILE, ENTRYPOINT_SH, tag, on_progress)
}

/// [`ensure_image`] with explicit content — split out so the integration
/// test can exercise the build machinery with a tiny Dockerfile instead of
/// the full agent image.
fn ensure_image_with(
    dockerfile: &str,
    entrypoint: &str,
    tag: &str,
    on_progress: Progress,
) -> Result<()> {
    static BUILD_LOCK: Mutex<()> = Mutex::new(());

    if image_exists(tag)? {
        return Ok(());
    }
    let _guard = BUILD_LOCK.lock().unwrap();
    // Re-check under the lock: a concurrent spawn may have just built it.
    if image_exists(tag)? {
        return Ok(());
    }

    tracing::info!(tag, "building agent docker image");
    // Build from a throwaway context dir holding exactly the two files —
    // nothing from the host repo can leak into the image.
    let ctx = tempfile::tempdir()?;
    std::fs::write(ctx.path().join("Dockerfile"), dockerfile)?;
    let entrypoint_path = ctx.path().join("entrypoint.sh");
    std::fs::write(&entrypoint_path, entrypoint)?;
    // COPY preserves the context file's mode. The embedded Dockerfile's
    // `RUN chmod +x` is what actually guarantees an executable entrypoint on
    // every host; setting the mode here too just keeps the copied layer's
    // metadata sane where the host supports it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&entrypoint_path, std::fs::Permissions::from_mode(0o755))?;
    }

    let args = build_args(tag, ctx.path());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // Broadcast the build lifecycle to the UI. `Started`/`Finished`/`Failed`
    // fire only here, where a build actually runs (a cached image returns above
    // without emitting), so the toast appears only for real builds. Each output
    // line is forwarded alongside the caller's own sink so the tracing
    // forwarder / test counter keep working unchanged.
    progress::emit(BuildEvent::Started);
    let forward = |line: &str| {
        on_progress(line);
        progress::emit(BuildEvent::Line {
            line: line.to_string(),
        });
    };
    let result = cli::run_docker_streaming(&args, BUILD_TIMEOUT, &forward);
    match &result {
        Ok(()) => progress::emit(BuildEvent::Finished),
        Err(e) => progress::emit(BuildEvent::Failed {
            error: e.to_string(),
        }),
    }
    result?;
    tracing::info!(tag, "agent docker image built");
    Ok(())
}

/// `docker build` argv for `tag` from context `ctx`.
fn build_args(tag: &str, ctx: &Path) -> Vec<String> {
    vec![
        "build".into(),
        "-t".into(),
        tag.into(),
        ctx.to_string_lossy().into_owned(),
    ]
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

    #[test]
    fn tag_is_content_addressed() {
        let tag = tag_for("FROM a\n", "#!/bin/sh\n");
        let (repo, hash) = tag.split_once(':').unwrap();
        assert_eq!(repo, "fletch-agent");
        assert_eq!(hash.len(), 12);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        // Deterministic, and any content change moves the tag.
        assert_eq!(tag, tag_for("FROM a\n", "#!/bin/sh\n"));
        assert_ne!(tag, tag_for("FROM b\n", "#!/bin/sh\n"));
        assert_ne!(tag, tag_for("FROM a\n", "#!/bin/bash\n"));
    }

    #[test]
    fn build_argv_shape() {
        assert_eq!(
            build_args("fletch-agent:abc123def456", Path::new("/tmp/ctx")),
            vec!["build", "-t", "fletch-agent:abc123def456", "/tmp/ctx"],
        );
    }

    /// The override path must not touch docker at all — it has to work (and
    /// return instantly) on machines where docker isn't even installed.
    #[test]
    fn override_image_skips_build_entirely() {
        let called = std::sync::atomic::AtomicBool::new(false);
        let progress = |_: &str| called.store(true, std::sync::atomic::Ordering::SeqCst);

        let image = resolve_image(Some("  ghcr.io/me/custom:1  "), &progress).unwrap();
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
        assert!(Some("   ")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none());
        assert!(image_tag().starts_with("fletch-agent:"));
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
        let tag = tag_for(dockerfile, ENTRYPOINT_SH);
        // Start clean so the build path actually runs.
        let _ = cli::run_docker(&["rmi", "-f", &tag], Duration::from_secs(30));

        let lines = std::sync::atomic::AtomicUsize::new(0);
        let progress = |_: &str| {
            lines.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        };
        ensure_image_with(dockerfile, ENTRYPOINT_SH, &tag, &progress).unwrap();
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
        ensure_image_with(dockerfile, ENTRYPOINT_SH, &tag, &progress).unwrap();
        assert_eq!(
            lines.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "an existing image must not rebuild",
        );

        let _ = cli::run_docker(&["rmi", "-f", &tag], Duration::from_secs(30));
    }
}
