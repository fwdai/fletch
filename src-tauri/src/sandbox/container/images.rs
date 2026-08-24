//! The embedded agent images' *content*: what a container runs, one image per
//! supported provider (see [`ContainerProvider`]).
//!
//! The Dockerfile and entrypoint are compiled into the binary and each image's
//! tag is derived from its content (`<repo>:<sha256[..12]>`, e.g.
//! `fletch-agent:…` for claude, `fletch-agent-codex:…` for codex), so shipping a
//! change to either automatically produces a new tag — the stale image is
//! never referenced again and the next spawn rebuilds. No version bookkeeping,
//! no manual invalidation. Provider images share their base layers (identical
//! `FROM` + apt step), so a second provider costs only its own install layer.
//!
//! Every embedded image also carries [`AGENT_IMAGE_LABEL`] so superseded images
//! (old hashes after a Dockerfile revision, untagged leftovers after a rebuild)
//! can be garbage-collected.
//!
//! Content only: nothing here talks to a container runtime. Building, inspecting,
//! the freshness TTL, and the GC all live in `sandbox::docker::image`.

use super::ContainerProvider;

/// The base every provider image builds on (enforced by a test over
/// [`image_spec`]). Named as a constant rather than left implicit in the five
/// Dockerfile literals because the reclamation path needs to know which tag a
/// `--pull` is able to move under us — see `image::reap_superseded_base`.
pub(crate) const BASE_IMAGE: &str = "node:22-slim";

/// The agent container image. Debian-slim keeps apt available for the tools
/// claude needs at runtime (`git`, `rg`, `jq`, `procps` for /proc-based
/// process introspection) while staying small; node 22 is claude-code's
/// supported runtime. The `chmod` guarantees the entrypoint is executable
/// regardless of the mode `COPY` picked up from the build context (context
/// files are written at build time on the host — see `image::ensure_image`).
pub(crate) const DOCKERFILE: &str = r#"FROM node:22-slim
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
pub(crate) const ENTRYPOINT_SH: &str = r#"#!/bin/sh
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
pub(crate) const CODEX_DOCKERFILE: &str = r#"FROM node:22-slim
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
pub(crate) const CODEX_ENTRYPOINT_SH: &str = r#"#!/bin/sh
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
pub(crate) const OPENCODE_DOCKERFILE: &str = r#"FROM node:22-slim
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
pub(crate) const OPENCODE_ENTRYPOINT_SH: &str = r#"#!/bin/sh
set -e
mkdir -p "$HOME"
exec "$@"
"#;

/// Pi's image. Shares [`DOCKERFILE`]'s base byte-for-byte for cache reuse; only
/// the install step differs. Pi ships as a pure-node CLI (`@earendil-works/
/// pi-coding-agent`, bin `pi` → a `dist/cli.js` launcher), so the same package
/// runs on every arch node:22-slim supports. Pi authenticates from the read-write
/// `~/.pi` mount (`~/.pi/agent/auth.json`) and/or a provider API-key env var.
pub(crate) const PI_DOCKERFILE: &str = r#"FROM node:22-slim
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
pub(crate) const PI_ENTRYPOINT_SH: &str = r#"#!/bin/sh
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
/// a forwarded `CURSOR_API_KEY` (see [`launch_auth`](super::launch_auth)):
/// `cursor-agent login` stores its tokens in the host OS keychain, which a
/// container can't read, so the image carries no provider config of its own.
pub(crate) const CURSOR_DOCKERFILE: &str = r#"FROM node:22-slim
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
pub(crate) const CURSOR_ENTRYPOINT_SH: &str = r#"#!/bin/sh
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
pub(crate) const AGENT_IMAGE_LABEL: &str = "fletch.agent";

/// The image build inputs for a provider: repo name plus the Dockerfile and
/// entrypoint whose combined content addresses the tag. Every spec's Dockerfile
/// carries `LABEL fletch.agent=<provider>` so the image GC can attribute it
/// (enforced by a test); each provider gets its own repo.
pub(crate) struct ImageSpec {
    pub(crate) repo: &'static str,
    pub(crate) dockerfile: &'static str,
    pub(crate) entrypoint: &'static str,
}

/// Per-provider image inputs. Claude's spec is the pre-existing embedded image
/// verbatim (see the byte-identity guard in tests); codex gets its own repo and
/// install step, sharing claude's base layers.
pub(crate) fn image_spec(provider: ContainerProvider) -> ImageSpec {
    match provider {
        ContainerProvider::Claude => ImageSpec {
            repo: "fletch-agent",
            dockerfile: DOCKERFILE,
            entrypoint: ENTRYPOINT_SH,
        },
        ContainerProvider::Codex => ImageSpec {
            repo: "fletch-agent-codex",
            dockerfile: CODEX_DOCKERFILE,
            entrypoint: CODEX_ENTRYPOINT_SH,
        },
        ContainerProvider::Opencode => ImageSpec {
            repo: "fletch-agent-opencode",
            dockerfile: OPENCODE_DOCKERFILE,
            entrypoint: OPENCODE_ENTRYPOINT_SH,
        },
        ContainerProvider::Pi => ImageSpec {
            repo: "fletch-agent-pi",
            dockerfile: PI_DOCKERFILE,
            entrypoint: PI_ENTRYPOINT_SH,
        },
        ContainerProvider::Cursor => ImageSpec {
            repo: "fletch-agent-cursor",
            dockerfile: CURSOR_DOCKERFILE,
            entrypoint: CURSOR_ENTRYPOINT_SH,
        },
    }
}

/// The content-addressed tag for a provider's embedded image.
pub(crate) fn image_tag(provider: ContainerProvider) -> String {
    let spec = image_spec(provider);
    tag_for(spec.repo, spec.dockerfile, spec.entrypoint)
}

/// The repo (image name without tag) of a provider's embedded image. With
/// [`image_tag`] this is the GC's vocabulary of Fletch-owned names: the repos
/// are chosen by this module and are not meaningful outside Fletch, so a
/// non-current tag under one of them is attributable to us even on legacy
/// images built before [`AGENT_IMAGE_LABEL`] existed.
pub(crate) fn image_repo(provider: ContainerProvider) -> &'static str {
    image_spec(provider).repo
}

/// `<repo>:<sha256(dockerfile + entrypoint)[..12]>` — 12 hex chars, the same
/// abbreviation depth docker itself uses for short ids. The hash covers only the
/// dockerfile+entrypoint content (not the repo), so claude's tail is unchanged
/// from before the repo argument existed as long as its content is.
pub(crate) fn tag_for(repo: &str, dockerfile: &str, entrypoint: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(dockerfile.as_bytes());
    hasher.update(entrypoint.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{repo}:{}", &hex[..12])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every provider image builds on [`BASE_IMAGE`]. This is the invariant
    /// `image::reap_superseded_base` rests on: it only ever inspects and
    /// reclaims that one tag, so a spec that quietly moved to a different base
    /// would orphan images nothing reclaims.
    #[test]
    fn every_spec_builds_on_the_declared_base() {
        let from = format!("FROM {BASE_IMAGE}\n");
        for provider in ContainerProvider::ALL {
            assert!(
                image_spec(provider).dockerfile.starts_with(&from),
                "{provider:?} must build FROM {BASE_IMAGE}",
            );
        }
    }

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
            tag_for("fletch-agent", "FROM a\n", "#!/bin/sh\n")
                .split_once(':')
                .unwrap()
                .1,
            tag_for("other", "FROM a\n", "#!/bin/sh\n")
                .split_once(':')
                .unwrap()
                .1,
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
        assert_eq!(
            ENTRYPOINT_SH, FROZEN_ENTRYPOINT,
            "claude entrypoint changed"
        );
        assert!(image_tag(ContainerProvider::Claude).starts_with("fletch-agent:"));
    }

    /// Codex gets its own repo and a distinct tag, and shares claude's base
    /// layers (identical `FROM` + apt line) so the cache is reused.
    #[test]
    fn codex_image_is_distinct_and_shares_base() {
        let codex = image_tag(ContainerProvider::Codex);
        assert!(codex.starts_with("fletch-agent-codex:"), "{codex}");
        assert_ne!(codex, image_tag(ContainerProvider::Claude));

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
        let claude = image_tag(ContainerProvider::Claude);
        let base = base_layers(DOCKERFILE);

        for (provider, prefix, pkg) in [
            (
                ContainerProvider::Opencode,
                "fletch-agent-opencode:",
                "opencode-ai",
            ),
            (
                ContainerProvider::Pi,
                "fletch-agent-pi:",
                "@earendil-works/pi-coding-agent",
            ),
        ] {
            let tag = image_tag(provider);
            assert!(tag.starts_with(prefix), "{tag}");
            assert_ne!(tag, claude);
            let dockerfile = image_spec(provider).dockerfile;
            assert_eq!(
                base_layers(dockerfile),
                base,
                "base layers must match for cache reuse"
            );
            assert!(dockerfile.contains(pkg), "{provider:?} must install {pkg}");
            // No cross-contamination with the other providers' install steps.
            assert!(!dockerfile.contains("claude-code"));
            assert!(!dockerfile.contains("@openai/codex"));
        }

        // The two new tags are distinct from each other, too.
        assert_ne!(
            image_tag(ContainerProvider::Opencode),
            image_tag(ContainerProvider::Pi)
        );
    }

    /// Cursor gets its own repo + distinct tag and shares claude's base layers
    /// (cache reuse) even though it installs via the official curl installer
    /// rather than npm — the base-sharing invariant is install-agnostic. No other
    /// provider's package leaks into the image.
    #[test]
    fn cursor_image_is_distinct_and_shares_base() {
        let cursor = image_tag(ContainerProvider::Cursor);
        assert!(cursor.starts_with("fletch-agent-cursor:"), "{cursor}");
        for other in [
            ContainerProvider::Claude,
            ContainerProvider::Codex,
            ContainerProvider::Opencode,
            ContainerProvider::Pi,
        ] {
            assert_ne!(
                cursor,
                image_tag(other),
                "cursor tag collides with {other:?}"
            );
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

    /// Every provider's Dockerfile carries `LABEL fletch.agent=<provider id>`
    /// — the GC's attribution authority. The value must round-trip through
    /// `ContainerProvider::from_id` so label values and provider ids can't drift.
    #[test]
    fn every_dockerfile_carries_the_agent_label() {
        for provider in ContainerProvider::ALL {
            let dockerfile = image_spec(provider).dockerfile;
            let value = dockerfile
                .lines()
                .find_map(|l| l.strip_prefix(&format!("LABEL {AGENT_IMAGE_LABEL}=")))
                .unwrap_or_else(|| {
                    panic!("{provider:?} Dockerfile is missing the fletch.agent label")
                });
            assert_eq!(
                ContainerProvider::from_id(value.trim()),
                Some(provider),
                "{provider:?} label value must be its provider id",
            );
            // `id()` is `from_id`'s inverse — the version trigger keys the
            // host probe and loop guard on it, so drift would silently
            // disable (or cross-wire) the trigger.
            assert_eq!(provider.id(), value.trim());
            assert_eq!(ContainerProvider::from_id(provider.id()), Some(provider));
        }
    }
}
