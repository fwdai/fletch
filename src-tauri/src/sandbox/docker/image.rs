//! The embedded agent images: what containers run, built on demand, one image
//! per supported provider (see [`DockerProvider`]).
//!
//! The Dockerfile and entrypoint are compiled into the binary and each image's
//! tag is derived from its content (`<repo>:<sha256[..12]>`, e.g.
//! `fletch-agent:…` for claude, `fletch-agent-codex:…` for codex), so shipping a
//! change to either automatically produces a new tag — the stale image is
//! never referenced again and the next spawn rebuilds. No version bookkeeping,
//! no manual invalidation. Provider images share their base layers (identical
//! `FROM` + apt step), so a second provider costs only its own install layer.
//!
//! Content addressing alone would freeze the *packages inside* an image
//! forever, though: every image installs "latest at build time" (npm installs,
//! cursor's installer), so a stable Dockerfile means a user's containerized CLI
//! never updates while the host CLI does. [`IMAGE_MAX_AGE`] fixes that with a
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
LABEL fletch.agent=claude
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
LABEL fletch.agent=codex
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

/// OpenCode's image. Shares [`DOCKERFILE`]'s base byte-for-byte (same `FROM
/// node:22-slim` and apt line) for layer-cache reuse; only the install step
/// differs (`opencode-ai`, whose `bin` resolves to a per-arch native binary via
/// npm optional deps — arm64 and x86-64 both publish one). OpenCode authenticates
/// from the read-write data-dir mount (its accounts DB / `auth.json`) and/or a
/// provider API-key env var, so the image carries no provider config.
pub const OPENCODE_DOCKERFILE: &str = r#"FROM node:22-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    git curl ca-certificates ripgrep jq procps \
 && rm -rf /var/lib/apt/lists/*
LABEL fletch.agent=opencode
RUN npm install -g opencode-ai
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
"#;

/// OpenCode's PID-1 shim: create `HOME` and exec. `opencode run --format json
/// --dangerously-skip-permissions` (see `agent::opencode_build_args`) is fully
/// non-interactive, and credentials arrive on the read-write data-dir mount or as
/// a forwarded API-key env var, so nothing is seeded here.
pub const OPENCODE_ENTRYPOINT_SH: &str = r#"#!/bin/sh
set -e
mkdir -p "$HOME"
exec "$@"
"#;

/// Pi's image. Shares [`DOCKERFILE`]'s base byte-for-byte for cache reuse; only
/// the install step differs. Pi ships as a pure-node CLI (`@earendil-works/
/// pi-coding-agent`, bin `pi` → a `dist/cli.js` launcher), so the same package
/// runs on every arch node:22-slim supports. Pi authenticates from the read-write
/// `~/.pi` mount (`~/.pi/agent/auth.json`) and/or a provider API-key env var.
pub const PI_DOCKERFILE: &str = r#"FROM node:22-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    git curl ca-certificates ripgrep jq procps \
 && rm -rf /var/lib/apt/lists/*
LABEL fletch.agent=pi
RUN npm install -g @earendil-works/pi-coding-agent
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
"#;

/// Pi's PID-1 shim: create `HOME` and exec. `pi -p --mode json` (see
/// `agent::pi_build_args`) runs one turn non-interactively and auto-runs tools;
/// credentials come from the mounted `~/.pi/agent/auth.json` or a forwarded
/// API-key env var, so nothing is seeded here.
pub const PI_ENTRYPOINT_SH: &str = r#"#!/bin/sh
set -e
mkdir -p "$HOME"
exec "$@"
"#;

/// Cursor's image. Shares [`DOCKERFILE`]'s base byte-for-byte (same `FROM
/// node:22-slim` and apt line) for layer-cache reuse; only the install step
/// differs. Unlike the other providers, cursor-agent ships not as an npm package
/// but via its official installer (`https://cursor.com/install`), which detects
/// `linux/arm64` and downloads a self-contained bundle (its own node runtime +
/// per-arch native modules) into `~/.local`; we symlink its `cursor-agent`
/// launcher onto PATH so the in-image `agent_bin` resolves, then run
/// `--version` so a build fails loudly if the installer ever relocates the
/// binary — `ln -s` happily creates a dangling link, and without the check
/// that drift would only surface as exit-127 launches. The installer pins
/// whatever version its script currently references — no worse than the `latest`
/// npm installs the other images use: the Dockerfile *text* is constant so the
/// content-addressed tag is stable, while a re-pull may fetch a newer bundle
/// (contents drift under a stable tag — an accepted, documented tradeoff shared
/// with every `npm install -g <pkg>` here). Cursor authenticates in-container from
/// a forwarded `CURSOR_API_KEY` (see [`super::engine`]): `cursor-agent login`
/// stores its tokens in the host OS keychain, which a container can't read, so the
/// image carries no provider config of its own.
pub const CURSOR_DOCKERFILE: &str = r#"FROM node:22-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    git curl ca-certificates ripgrep jq procps \
 && rm -rf /var/lib/apt/lists/*
LABEL fletch.agent=cursor
RUN curl -fsSL https://cursor.com/install | bash \
 && ln -s /root/.local/bin/cursor-agent /usr/local/bin/cursor-agent \
 && cursor-agent --version
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
"#;

/// Cursor's PID-1 shim: create `HOME` and exec. `cursor-agent -p --output-format
/// stream-json --force --trust` (see `agent::cursor_build_args`) runs one turn
/// non-interactively; credentials arrive as the forwarded `CURSOR_API_KEY` env
/// var and its transcripts land on the read-write `~/.cursor` mount, so nothing is
/// seeded here.
pub const CURSOR_ENTRYPOINT_SH: &str = r#"#!/bin/sh
set -e
mkdir -p "$HOME"
exec "$@"
"#;

/// Label key baked into every embedded agent image (`LABEL
/// fletch.agent=<provider>` in the Dockerfiles above). It is the image GC's
/// authority: only images carrying it — or, transitionally, pre-label images
/// in a Fletch-owned repo — are ever candidates for removal (see
/// `cleanup::sweep_stale_images`). The user's `docker_image` override never
/// gets the label (it is never built by us), so it can't be attributed to
/// Fletch and is structurally safe from the GC.
pub const AGENT_IMAGE_LABEL: &str = "fletch.agent";

/// Dogma: **an agent image is never older than a week.** The images install
/// "latest at build time" (npm installs, cursor's installer), so their
/// contents freeze at build; this TTL bounds that freeze. An image past the
/// TTL still serves the current launch — freshness is a background concern,
/// never a launch blocker — and is rebuilt under the same tag off-thread
/// (see [`refresh_in_background_if_stale`]). Deliberately not a setting.
pub const IMAGE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The image build inputs for a provider: repo name plus the Dockerfile and
/// entrypoint whose combined content addresses the tag. Every spec's Dockerfile
/// carries `LABEL fletch.agent=<provider>` so the image GC can attribute it
/// (enforced by a test); each provider gets its own repo.
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
        DockerProvider::Opencode => ImageSpec {
            repo: "fletch-agent-opencode",
            dockerfile: OPENCODE_DOCKERFILE,
            entrypoint: OPENCODE_ENTRYPOINT_SH,
        },
        DockerProvider::Pi => ImageSpec {
            repo: "fletch-agent-pi",
            dockerfile: PI_DOCKERFILE,
            entrypoint: PI_ENTRYPOINT_SH,
        },
        DockerProvider::Cursor => ImageSpec {
            repo: "fletch-agent-cursor",
            dockerfile: CURSOR_DOCKERFILE,
            entrypoint: CURSOR_ENTRYPOINT_SH,
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

/// The repo (image name without tag) of a provider's embedded image. With
/// [`image_tag`] this is the GC's vocabulary of Fletch-owned names: the repos
/// are chosen by this module and are not meaningful outside Fletch, so a
/// non-current tag under one of them is attributable to us even on legacy
/// images built before [`AGENT_IMAGE_LABEL`] existed.
pub fn image_repo(provider: DockerProvider) -> &'static str {
    image_spec(provider).repo
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
/// key: a non-empty override is returned verbatim (no build, no inspect, no
/// TTL, no version check — the user owns that image's lifecycle); otherwise
/// the embedded image is built if missing, refreshed in the background if
/// older than [`IMAGE_MAX_AGE`] or version-divergent from the host CLI, and
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
    progress::emit(BuildEvent::Started);
    let forward = |line: &str| {
        on_progress(line);
        progress::emit(BuildEvent::Line {
            line: line.to_string(),
        });
    };
    let result = run_build(dockerfile, entrypoint, tag, false, &forward);
    match &result {
        Ok(()) => progress::emit(BuildEvent::Finished),
        Err(e) => progress::emit(BuildEvent::Failed {
            error: e.to_string(),
        }),
    }
    result?;
    tracing::info!(tag, "agent docker image built");
    Ok(false)
}

/// Write the two-file build context and run `docker build -t tag`, streaming
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

    let args = build_args(tag, ctx.path(), no_cache);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    cli::run_docker_streaming(&args, BUILD_TIMEOUT, on_line)
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

/// TTL verdict for an image's `.Created` timestamp against [`IMAGE_MAX_AGE`].
/// Pure — `now` is injected so tests use fixed instants. `Unknown` (an
/// unparseable timestamp) is deliberately its own state: the caller treats it
/// as fresh, because rebuilding on unparseable metadata would rebuild on
/// *every* resolution forever (the rebuilt image's metadata would presumably
/// parse no better if the daemon's format changed under us).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Freshness {
    Fresh,
    Stale,
    Unknown,
}

/// Classify a raw `docker image inspect --format {{.Created}}` value (RFC3339,
/// e.g. `2026-07-01T12:00:00.000000000Z`).
fn classify_freshness(created_raw: &str, now: chrono::DateTime<chrono::Utc>) -> Freshness {
    let Ok(created) = chrono::DateTime::parse_from_rfc3339(created_raw.trim()) else {
        return Freshness::Unknown;
    };
    // A negative age (clock skew, image "from the future") compares below any
    // positive TTL and lands on Fresh — the right bias for bad clocks.
    let max_age = chrono::Duration::from_std(IMAGE_MAX_AGE).expect("TTL fits chrono::Duration");
    if now.signed_duration_since(created) > max_age {
        Freshness::Stale
    } else {
        Freshness::Fresh
    }
}

/// Why a background refresh rebuild was kicked — log attribution, plus the
/// version trigger's loop-guard bookkeeping on success.
enum RefreshReason {
    /// The image's build date passed [`IMAGE_MAX_AGE`].
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
    spawn_refresh_rebuild(provider, tag.to_string(), RefreshReason::VersionMismatch { guard_pair });
}

/// Pure core of the version-mismatch trigger: refresh only when both sides
/// are known, differ (plain `!=` — deliberately no semver ordering: "newer"
/// is not computable across five vendors' formats, and parity is the actual
/// goal), and this pairing hasn't already been attempted (rebuilding can't
/// fix a host that's simply pinned away from the registry's latest).
fn version_refresh_wanted(
    host: Option<&str>,
    container: Option<&str>,
    already_attempted: bool,
) -> bool {
    match (host, container) {
        (Some(h), Some(c)) => !already_attempted && h != c,
        _ => false,
    }
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
    cache.lock().unwrap().insert(image_id.to_string(), version.clone());
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
        &["run", "--rm", "--label", &pid_label, tag, provider.image_bin(), "--version"],
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
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
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

    /// Golden guard: claude's image content is pinned byte-for-byte so an
    /// *accidental* edit can't silently move the content-addressed tag and
    /// force every user through a cold rebuild. Deliberate changes update the
    /// frozen bytes here and record why:
    ///
    /// - image-lifecycle PR: added `LABEL fletch.agent=claude` so the image GC
    ///   can attribute Fletch's images by label instead of guessing from
    ///   names. One planned rehash → one rebuild for every user on update,
    ///   accepted (and the GC reaps the superseded image).
    #[test]
    fn claude_image_is_unchanged() {
        // Frozen bytes (do not "fix" to match an accidentally changed constant
        // — update the constant back instead; deliberate changes update these
        // bytes and the doc comment above).
        const FROZEN_DOCKERFILE: &str = "FROM node:22-slim\nRUN apt-get update && apt-get install -y --no-install-recommends \\\n    git curl ca-certificates ripgrep jq procps \\\n && rm -rf /var/lib/apt/lists/*\nLABEL fletch.agent=claude\nRUN npm install -g @anthropic-ai/claude-code\nCOPY entrypoint.sh /entrypoint.sh\nRUN chmod +x /entrypoint.sh\nENTRYPOINT [\"/entrypoint.sh\"]\n";
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
        // (The per-provider `LABEL fletch.agent=…` sits after the base and is
        // deliberately excluded — it differs by construction.)
        assert_eq!(
            base_layers(DOCKERFILE),
            base_layers(CODEX_DOCKERFILE),
            "base layers must match for cache reuse"
        );
        // Provider-specific install step differs.
        assert!(CODEX_DOCKERFILE.contains("@openai/codex"));
        assert!(!CODEX_DOCKERFILE.contains("claude-code"));
    }

    /// The base layers (`FROM` + the shared apt step, through the apt-list
    /// cleanup) that every provider image must share byte-for-byte so Docker's
    /// layer cache is reused. Stops at the apt cleanup rather than the install
    /// `RUN` so it's install-agnostic — npm-installed providers and cursor's
    /// curl-installer image both compare equal on the base.
    fn base_layers(dockerfile: &str) -> Vec<&str> {
        let mut out = Vec::new();
        for line in dockerfile.lines() {
            out.push(line);
            if line.contains("rm -rf /var/lib/apt/lists") {
                break;
            }
        }
        out
    }

    /// OpenCode and Pi each get their own repo + distinct tag, share claude's base
    /// layers (cache reuse), and install only their own package — no other
    /// provider's CLI leaks into either image.
    #[test]
    fn opencode_and_pi_images_are_distinct_and_share_base() {
        let claude = image_tag(DockerProvider::Claude);
        let base = base_layers(DOCKERFILE);

        for (provider, prefix, pkg) in [
            (DockerProvider::Opencode, "fletch-agent-opencode:", "opencode-ai"),
            (DockerProvider::Pi, "fletch-agent-pi:", "@earendil-works/pi-coding-agent"),
        ] {
            let tag = image_tag(provider);
            assert!(tag.starts_with(prefix), "{tag}");
            assert_ne!(tag, claude);
            let dockerfile = image_spec(provider).dockerfile;
            assert_eq!(base_layers(dockerfile), base, "base layers must match for cache reuse");
            assert!(dockerfile.contains(pkg), "{provider:?} must install {pkg}");
            // No cross-contamination with the other providers' install steps.
            assert!(!dockerfile.contains("claude-code"));
            assert!(!dockerfile.contains("@openai/codex"));
        }

        // The two new tags are distinct from each other, too.
        assert_ne!(image_tag(DockerProvider::Opencode), image_tag(DockerProvider::Pi));
    }

    /// Cursor gets its own repo + distinct tag and shares claude's base layers
    /// (cache reuse) even though it installs via the official curl installer
    /// rather than npm — the base-sharing invariant is install-agnostic. No other
    /// provider's package leaks into the image.
    #[test]
    fn cursor_image_is_distinct_and_shares_base() {
        let cursor = image_tag(DockerProvider::Cursor);
        assert!(cursor.starts_with("fletch-agent-cursor:"), "{cursor}");
        for other in [
            DockerProvider::Claude,
            DockerProvider::Codex,
            DockerProvider::Opencode,
            DockerProvider::Pi,
        ] {
            assert_ne!(cursor, image_tag(other), "cursor tag collides with {other:?}");
        }
        // Base (FROM + apt) byte-identical despite the curl-installer install step.
        assert_eq!(
            base_layers(CURSOR_DOCKERFILE),
            base_layers(DOCKERFILE),
            "base layers must match for cache reuse",
        );
        // Installs cursor-agent via its official installer; no other provider's pkg.
        assert!(CURSOR_DOCKERFILE.contains("cursor.com/install"));
        assert!(CURSOR_DOCKERFILE.contains("cursor-agent"));
        assert!(!CURSOR_DOCKERFILE.contains("claude-code"));
        assert!(!CURSOR_DOCKERFILE.contains("@openai/codex"));
        assert!(!CURSOR_DOCKERFILE.contains("opencode-ai"));
        assert!(!CURSOR_DOCKERFILE.contains("pi-coding-agent"));
    }

    #[test]
    fn build_argv_shape() {
        // `--pull` on every build, `--no-cache` on refresh rebuilds only: see
        // the `build_args` doc — neither a stale cached base nor a cached
        // install layer may defeat the freshness the rebuilds exist for.
        assert_eq!(
            build_args("fletch-agent:abc123def456", Path::new("/tmp/ctx"), false),
            vec!["build", "--pull", "-t", "fletch-agent:abc123def456", "/tmp/ctx"],
        );
        assert_eq!(
            build_args("fletch-agent:abc123def456", Path::new("/tmp/ctx"), true),
            vec!["build", "--pull", "--no-cache", "-t", "fletch-agent:abc123def456", "/tmp/ctx"],
        );
    }

    /// Every provider's Dockerfile carries `LABEL fletch.agent=<provider id>`
    /// — the GC's attribution authority. The value must round-trip through
    /// `DockerProvider::from_id` so label values and provider ids can't drift.
    #[test]
    fn every_dockerfile_carries_the_agent_label() {
        for provider in DockerProvider::ALL {
            let dockerfile = image_spec(provider).dockerfile;
            let value = dockerfile
                .lines()
                .find_map(|l| l.strip_prefix(&format!("LABEL {AGENT_IMAGE_LABEL}=")))
                .unwrap_or_else(|| panic!("{provider:?} Dockerfile is missing the fletch.agent label"));
            assert_eq!(
                DockerProvider::from_id(value.trim()),
                Some(provider),
                "{provider:?} label value must be its provider id",
            );
            // `id()` is `from_id`'s inverse — the version trigger keys the
            // host probe and loop guard on it, so drift would silently
            // disable (or cross-wire) the trigger.
            assert_eq!(provider.id(), value.trim());
            assert_eq!(DockerProvider::from_id(provider.id()), Some(provider));
        }
    }

    /// The version-mismatch trigger's pure core: refresh only on a known,
    /// unequal, not-yet-attempted pairing. Plain `!=`, no semver ordering.
    #[test]
    fn version_refresh_decision() {
        // Mismatch → refresh.
        assert!(version_refresh_wanted(Some("v2.0.1"), Some("v2.0.0"), false));
        // Direction doesn't matter (no ordering): host older also fires once.
        assert!(version_refresh_wanted(Some("v1.0.0"), Some("v2.0.0"), false));
        // Match → fresh.
        assert!(!version_refresh_wanted(Some("v2.0.0"), Some("v2.0.0"), false));
        // No host CLI (not installed / probe failed) → inert.
        assert!(!version_refresh_wanted(None, Some("v2.0.0"), false));
        // Container version unknown (post-build probe failed) → inert.
        assert!(!version_refresh_wanted(Some("v2.0.0"), None, false));
        assert!(!version_refresh_wanted(None, None, false));
        // Already-attempted pairing → inert (pinned host must not loop).
        assert!(!version_refresh_wanted(Some("v2.0.1"), Some("v2.0.0"), true));
    }

    /// The TTL decision's pure core, with fixed instants: inside the window →
    /// fresh, past it → stale, unparseable → unknown (treated as fresh by the
    /// caller — never rebuild-loop on bad metadata), and a future timestamp
    /// (clock skew) → fresh.
    #[test]
    fn freshness_classification() {
        use chrono::{TimeZone, Utc};
        let now = Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap();

        // Docker's actual format: RFC3339 with nanoseconds and Z.
        assert_eq!(
            classify_freshness("2026-07-07T12:00:00.123456789Z", now),
            Freshness::Fresh,
            "one day old is fresh",
        );
        assert_eq!(
            classify_freshness("2026-07-01T12:00:00Z", now),
            Freshness::Fresh,
            "exactly the TTL boundary is still fresh (strictly-older rebuilds)",
        );
        assert_eq!(
            classify_freshness("2026-07-01T11:59:59Z", now),
            Freshness::Stale,
            "past the TTL is stale",
        );
        assert_eq!(
            classify_freshness("2026-01-01T00:00:00Z", now),
            Freshness::Stale,
            "months old is stale",
        );
        assert_eq!(
            classify_freshness("2026-08-01T00:00:00Z", now),
            Freshness::Fresh,
            "a future build date (clock skew) is fresh, not stale",
        );
        assert_eq!(
            classify_freshness("not-a-timestamp", now),
            Freshness::Unknown,
        );
        assert_eq!(classify_freshness("", now), Freshness::Unknown);
        // Whitespace from the CLI pipe is tolerated.
        assert_eq!(
            classify_freshness("  2026-07-07T12:00:00Z\n", now),
            Freshness::Fresh,
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
