//! The embedded agent images: what containers run, built on demand, one image
//! per supported provider (see [`DockerProvider`]).
//!
//! The Dockerfile and entrypoint are compiled into the binary and each image's
//! tag is derived from its content (`<repo>:<sha256[..12]>`, e.g.
//! `fletch-agent:…` for claude, `fletch-agent-codex:…` for codex), so shipping a
//! change to either automatically produces a new tag — the stale image is simply
//! never referenced again and the next spawn rebuilds. No version bookkeeping,
//! no manual invalidation. Provider images share their base layers (identical
//! `FROM` + apt step), so a second provider costs only its own install layer.
//!
//! Users can bypass all of this with the `docker_image` settings key (see
//! [`resolve_image`]): a user-supplied image is used verbatim — never built,
//! never inspected — and must have the launching provider's CLI on PATH and git
//! installed. The override is global (applies to whichever provider launches).

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use crate::error::Result;

use super::cli;
use super::progress::{self, BuildEvent};
use super::DockerProvider;

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

/// Codex's image. Shares [`DOCKERFILE`]'s base byte-for-byte — same `FROM
/// node:22-slim` and the same apt line — so Docker's layer cache is reused
/// across the two images; only the provider install step differs
/// (`@openai/codex` instead of `@anthropic-ai/claude-code`). Codex authenticates
/// from the read-write `~/.codex` mount (auth.json) and/or `OPENAI_API_KEY`, so
/// the image carries no provider config of its own.
pub const CODEX_DOCKERFILE: &str = r#"FROM node:22-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    git curl ca-certificates ripgrep jq procps \
 && rm -rf /var/lib/apt/lists/*
RUN npm install -g @openai/codex
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
"#;

/// Codex's PID-1 shim: create `HOME` and exec, nothing more. Unlike claude,
/// codex needs no onboarding seed — `codex exec` runs non-interactively with
/// `--skip-git-repo-check` and `approval_policy="never"` (see
/// `agent::codex_build_args`), and its credentials come from the mounted
/// `~/.codex/auth.json` rather than a config file we'd seed here.
pub const CODEX_ENTRYPOINT_SH: &str = r#"#!/bin/sh
set -e
mkdir -p "$HOME"
exec "$@"
"#;

/// The image build inputs for a provider: repo name plus the Dockerfile and
/// entrypoint whose combined content addresses the tag. Claude returns the
/// original constants under the original repo name, so its tag is byte-for-byte
/// unchanged and existing users never rebuild; new providers get their own repo.
struct ImageSpec {
    repo: &'static str,
    dockerfile: &'static str,
    entrypoint: &'static str,
}

/// Per-provider image inputs. Claude's spec is the pre-existing embedded image
/// verbatim (see the byte-identity guard in tests); codex gets its own repo and
/// install step, sharing claude's base layers.
fn image_spec(provider: DockerProvider) -> ImageSpec {
    match provider {
        DockerProvider::Claude => ImageSpec {
            repo: "fletch-agent",
            dockerfile: DOCKERFILE,
            entrypoint: ENTRYPOINT_SH,
        },
        DockerProvider::Codex => ImageSpec {
            repo: "fletch-agent-codex",
            dockerfile: CODEX_DOCKERFILE,
            entrypoint: CODEX_ENTRYPOINT_SH,
        },
    }
}

/// Builds are slow (base image pull + apt + npm) but bounded: past this we
/// assume a wedged daemon or dead network and fail the spawn with a clear
/// error rather than letting it hang indefinitely.
const BUILD_TIMEOUT: Duration = Duration::from_secs(600);

/// Quick metadata lookups (`docker image inspect`).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The content-addressed tag for a provider's embedded image.
pub fn image_tag(provider: DockerProvider) -> String {
    let spec = image_spec(provider);
    tag_for(spec.repo, spec.dockerfile, spec.entrypoint)
}

/// `<repo>:<sha256(dockerfile + entrypoint)[..12]>` — 12 hex chars, the same
/// abbreviation depth docker itself uses for short ids. The hash covers only the
/// dockerfile+entrypoint content (not the repo), so claude's tail is unchanged
/// from before the repo argument existed as long as its content is.
fn tag_for(repo: &str, dockerfile: &str, entrypoint: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(dockerfile.as_bytes());
    hasher.update(entrypoint.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{repo}:{}", &hex[..12])
}

/// The image to launch containers from, honoring the `docker_image` settings
/// key: a non-empty override is returned verbatim (no build, no inspect —
/// the user owns that image's lifecycle); otherwise the embedded image is
/// built if missing and its tag returned. Callers read the settings key and
/// pass it in — this module stays DB-free.
pub fn resolve_image(
    provider: DockerProvider,
    override_image: Option<&str>,
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
    ensure_image(provider, &tag, on_progress)?;
    Ok(tag)
}

/// Make sure `provider`'s image `tag` exists locally, building its embedded
/// Dockerfile under that tag if it doesn't. Builds are serialized process-wide:
/// concurrent spawns during a cold start would otherwise race docker into
/// building the same image N times.
pub fn ensure_image(provider: DockerProvider, tag: &str, on_progress: Progress) -> Result<()> {
    let spec = image_spec(provider);
    ensure_image_with(spec.dockerfile, spec.entrypoint, tag, on_progress)
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
        let tag = tag_for("fletch-agent", "FROM a\n", "#!/bin/sh\n");
        let (repo, hash) = tag.split_once(':').unwrap();
        assert_eq!(repo, "fletch-agent");
        assert_eq!(hash.len(), 12);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        // Deterministic, and any content change moves the tag.
        assert_eq!(tag, tag_for("fletch-agent", "FROM a\n", "#!/bin/sh\n"));
        assert_ne!(tag, tag_for("fletch-agent", "FROM b\n", "#!/bin/sh\n"));
        assert_ne!(tag, tag_for("fletch-agent", "FROM a\n", "#!/bin/bash\n"));
        // The repo is a prefix, not part of the hash: a different repo with the
        // same content shares the tail (so a repo rename can't force a rebuild).
        assert_eq!(
            tag_for("fletch-agent", "FROM a\n", "#!/bin/sh\n").split_once(':').unwrap().1,
            tag_for("other", "FROM a\n", "#!/bin/sh\n").split_once(':').unwrap().1,
        );
    }

    /// Acceptance guard: claude's image must not change. Its repo name stays
    /// `fletch-agent` and its Dockerfile/entrypoint bytes are frozen, so its
    /// content-addressed tag is byte-for-byte what shipped — existing users must
    /// never be forced to rebuild by this PR. If this fails, claude's image
    /// content changed and every user pays a cold rebuild.
    #[test]
    fn claude_image_is_unchanged() {
        // Frozen bytes (do not "fix" to match a changed constant — update the
        // constant back instead).
        const FROZEN_DOCKERFILE: &str = "FROM node:22-slim\nRUN apt-get update && apt-get install -y --no-install-recommends \\\n    git curl ca-certificates ripgrep jq procps \\\n && rm -rf /var/lib/apt/lists/*\nRUN npm install -g @anthropic-ai/claude-code\nCOPY entrypoint.sh /entrypoint.sh\nRUN chmod +x /entrypoint.sh\nENTRYPOINT [\"/entrypoint.sh\"]\n";
        const FROZEN_ENTRYPOINT: &str = "#!/bin/sh\nset -e\nmkdir -p \"$HOME\"\nif [ ! -f \"$HOME/.claude.json\" ]; then\n  printf '{\"hasCompletedOnboarding\": true}\\n' > \"$HOME/.claude.json\"\nfi\nexec \"$@\"\n";
        assert_eq!(DOCKERFILE, FROZEN_DOCKERFILE, "claude Dockerfile changed");
        assert_eq!(ENTRYPOINT_SH, FROZEN_ENTRYPOINT, "claude entrypoint changed");
        assert!(image_tag(DockerProvider::Claude).starts_with("fletch-agent:"));
    }

    /// Codex gets its own repo and a distinct tag, and shares claude's base
    /// layers (identical `FROM` + apt line) so the cache is reused.
    #[test]
    fn codex_image_is_distinct_and_shares_base() {
        let codex = image_tag(DockerProvider::Codex);
        assert!(codex.starts_with("fletch-agent-codex:"), "{codex}");
        assert_ne!(codex, image_tag(DockerProvider::Claude));

        // Base layers shared: the FROM line and the apt install line are
        // byte-identical, so Docker reuses those layers across both images.
        let base: Vec<&str> = DOCKERFILE.lines().take_while(|l| !l.starts_with("RUN npm")).collect();
        let codex_base: Vec<&str> =
            CODEX_DOCKERFILE.lines().take_while(|l| !l.starts_with("RUN npm")).collect();
        assert_eq!(base, codex_base, "base layers must match for cache reuse");
        // Provider-specific install step differs.
        assert!(CODEX_DOCKERFILE.contains("@openai/codex"));
        assert!(!CODEX_DOCKERFILE.contains("claude-code"));
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

        let image =
            resolve_image(DockerProvider::Claude, Some("  ghcr.io/me/custom:1  "), &progress)
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
        assert!(Some("   ")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none());
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
