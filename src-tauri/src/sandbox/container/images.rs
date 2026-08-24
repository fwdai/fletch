//! The embedded agent images' *content*: what a container runs, one image per
//! [`ContainerProvider`].
//!
//! Each tag is content-addressed (`<repo>:<sha256(dockerfile+entrypoint)[..12]>`),
//! so editing either file produces a new tag and the next spawn rebuilds — no
//! version bookkeeping. Provider images share base layers byte-for-byte for
//! cache reuse, and all carry [`AGENT_IMAGE_LABEL`] so the GC can attribute them.
//!
//! Content only: nothing here talks to a runtime. Building, inspecting and the
//! GC live in `sandbox::docker::image` / `sandbox::podman::image`.

use super::ContainerProvider;

/// The base every provider image builds on (enforced by a test over
/// [`image_spec`]). A constant because `image::reap_superseded_base` reclaims
/// exactly this one tag — the only tag a `--pull` can move under us.
pub(crate) const BASE_IMAGE: &str = "node:22-slim";

/// Claude's image. `procps` is there for /proc-based process introspection; the
/// `chmod` makes the entrypoint executable whatever mode `COPY` picked up from
/// the host-written build context.
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

/// Claude's PID-1 shim. `HOME=<host home>` doesn't exist in the image, so it is
/// created here along with the onboarding seed. The seed stays
/// container-ephemeral: bind-mounting the real `~/.claude.json` would break on
/// claude's atomic rename-replace writes.
pub(crate) const ENTRYPOINT_SH: &str = r#"#!/bin/sh
set -e
mkdir -p "$HOME"
if [ ! -f "$HOME/.claude.json" ]; then
  printf '{"hasCompletedOnboarding": true}\n' > "$HOME/.claude.json"
fi
exec "$@"
"#;

/// Codex's image. Base is byte-identical to [`DOCKERFILE`]'s for layer-cache
/// reuse; only the install step differs. Auth comes from the read-write
/// `~/.codex` mount and/or `OPENAI_API_KEY`, so no provider config is baked in.
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

/// Codex's PID-1 shim: create `HOME` and exec. No onboarding seed — `codex exec`
/// (see `agent::codex_build_args`) is already non-interactive.
pub(crate) const CODEX_ENTRYPOINT_SH: &str = r#"#!/bin/sh
set -e
mkdir -p "$HOME"
exec "$@"
"#;

/// OpenCode's image. Base byte-identical to [`DOCKERFILE`]'s for cache reuse.
/// `opencode-ai`'s bin resolves to a per-arch native binary via npm optional
/// deps (arm64 and x86-64 both publish one). Auth comes from the read-write
/// data-dir mount and/or an API-key env var, so no provider config is baked in.
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

/// OpenCode's PID-1 shim: create `HOME` and exec. Nothing to seed — see
/// `agent::opencode_build_args`.
pub(crate) const OPENCODE_ENTRYPOINT_SH: &str = r#"#!/bin/sh
set -e
mkdir -p "$HOME"
exec "$@"
"#;

/// Pi's image. Base byte-identical to [`DOCKERFILE`]'s for cache reuse. Pi is a
/// pure-node CLI, so one package covers every arch node:22-slim supports. Auth
/// comes from the read-write `~/.pi` mount and/or an API-key env var.
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

/// Pi's PID-1 shim: create `HOME` and exec. Nothing to seed — see
/// `agent::pi_build_args`.
pub(crate) const PI_ENTRYPOINT_SH: &str = r#"#!/bin/sh
set -e
mkdir -p "$HOME"
exec "$@"
"#;

/// Cursor's image. Base byte-identical to [`DOCKERFILE`]'s for cache reuse;
/// cursor-agent installs via its official installer into `~/.local`, so the
/// symlink puts it on PATH for the in-image `agent_bin`. The trailing
/// `--version` is load-bearing: `ln -s` creates dangling links happily, so
/// without it an installer relocation would only surface as exit-127 launches.
/// Auth is the forwarded `CURSOR_API_KEY` (see [`launch_auth`](super::launch_auth))
/// — `cursor-agent login` writes the host keychain, which a container can't read.
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

/// Cursor's PID-1 shim: create `HOME` and exec. Nothing to seed — see
/// `agent::cursor_build_args`.
pub(crate) const CURSOR_ENTRYPOINT_SH: &str = r#"#!/bin/sh
set -e
mkdir -p "$HOME"
exec "$@"
"#;

/// Label key baked into every embedded agent image — the image GC's ownership
/// authority: only labeled images (or, transitionally, pre-label images in a
/// Fletch-owned repo) are removal candidates. A user's image override never
/// carries it, so it is structurally safe from the GC.
pub(crate) const AGENT_IMAGE_LABEL: &str = "fletch.agent";

/// A provider's image build inputs: its own repo, plus the Dockerfile and
/// entrypoint whose combined content addresses the tag.
pub(crate) struct ImageSpec {
    pub(crate) repo: &'static str,
    pub(crate) dockerfile: &'static str,
    pub(crate) entrypoint: &'static str,
}

/// Per-provider image inputs.
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

/// The repo (image name without tag) of a provider's embedded image. These
/// names are meaningless outside Fletch, which is what lets the GC attribute
/// images built before [`AGENT_IMAGE_LABEL`] existed.
pub(crate) fn image_repo(provider: ContainerProvider) -> &'static str {
    image_spec(provider).repo
}

/// Write a build context holding exactly the two embedded files, so nothing
/// from the host repo can leak into the image. Callers own the throwaway `dir`.
/// The mode set here is cosmetic — the Dockerfiles' `RUN chmod +x` is what
/// actually guarantees an executable entrypoint on every host.
pub(crate) fn write_build_context(
    dir: &std::path::Path,
    dockerfile: &str,
    entrypoint: &str,
) -> std::io::Result<()> {
    std::fs::write(dir.join("Dockerfile"), dockerfile)?;
    let entrypoint_path = dir.join("entrypoint.sh");
    std::fs::write(&entrypoint_path, entrypoint)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&entrypoint_path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// `<repo>:<sha256(dockerfile + entrypoint)[..12]>`. The repo is deliberately
/// outside the hash, so renaming a repo never forces a rebuild.
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

    /// `image::reap_superseded_base` reclaims only [`BASE_IMAGE`], so a spec on
    /// a different base would orphan images nothing reclaims.
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

        assert_eq!(tag, tag_for("fletch-agent", "FROM a\n", "#!/bin/sh\n"));
        assert_ne!(tag, tag_for("fletch-agent", "FROM b\n", "#!/bin/sh\n"));
        assert_ne!(tag, tag_for("fletch-agent", "FROM a\n", "#!/bin/bash\n"));
        // The repo is a prefix, not part of the hash: a rename can't force a rebuild.
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

    /// Golden guard: an accidental edit here moves the content-addressed tag
    /// and puts every user through a cold rebuild. Deliberate changes update
    /// the frozen bytes below; accidental ones revert the constant instead.
    #[test]
    fn claude_image_is_unchanged() {
        const FROZEN_DOCKERFILE: &str = "FROM node:22-slim\nRUN apt-get update && apt-get install -y --no-install-recommends \\\n    git curl ca-certificates ripgrep jq procps \\\n && rm -rf /var/lib/apt/lists/*\nLABEL fletch.agent=claude\nRUN npm install -g @anthropic-ai/claude-code\nCOPY entrypoint.sh /entrypoint.sh\nRUN chmod +x /entrypoint.sh\nENTRYPOINT [\"/entrypoint.sh\"]\n";
        const FROZEN_ENTRYPOINT: &str = "#!/bin/sh\nset -e\nmkdir -p \"$HOME\"\nif [ ! -f \"$HOME/.claude.json\" ]; then\n  printf '{\"hasCompletedOnboarding\": true}\\n' > \"$HOME/.claude.json\"\nfi\nexec \"$@\"\n";
        assert_eq!(DOCKERFILE, FROZEN_DOCKERFILE, "claude Dockerfile changed");
        assert_eq!(
            ENTRYPOINT_SH, FROZEN_ENTRYPOINT,
            "claude entrypoint changed"
        );
        assert!(image_tag(ContainerProvider::Claude).starts_with("fletch-agent:"));
    }

    #[test]
    fn codex_image_is_distinct_and_shares_base() {
        let codex = image_tag(ContainerProvider::Codex);
        assert!(codex.starts_with("fletch-agent-codex:"), "{codex}");
        assert_ne!(codex, image_tag(ContainerProvider::Claude));

        assert_eq!(
            base_layers(DOCKERFILE),
            base_layers(CODEX_DOCKERFILE),
            "base layers must match for cache reuse"
        );
        assert!(CODEX_DOCKERFILE.contains("@openai/codex"));
        assert!(!CODEX_DOCKERFILE.contains("claude-code"));
    }

    /// The lines every provider image must share byte-for-byte for layer-cache
    /// reuse. Stops at the apt cleanup, not the install `RUN`, so it stays
    /// install-agnostic (cursor installs via curl, the rest via npm).
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
            assert!(!dockerfile.contains("claude-code"));
            assert!(!dockerfile.contains("@openai/codex"));
        }

        assert_ne!(
            image_tag(ContainerProvider::Opencode),
            image_tag(ContainerProvider::Pi)
        );
    }

    /// Base sharing holds even though cursor installs via curl, not npm.
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
        assert_eq!(
            base_layers(CURSOR_DOCKERFILE),
            base_layers(DOCKERFILE),
            "base layers must match for cache reuse",
        );
        assert!(CURSOR_DOCKERFILE.contains("cursor.com/install"));
        assert!(CURSOR_DOCKERFILE.contains("cursor-agent"));
        assert!(!CURSOR_DOCKERFILE.contains("claude-code"));
        assert!(!CURSOR_DOCKERFILE.contains("@openai/codex"));
        assert!(!CURSOR_DOCKERFILE.contains("opencode-ai"));
        assert!(!CURSOR_DOCKERFILE.contains("pi-coding-agent"));
    }

    /// The label value must round-trip through `ContainerProvider::from_id`, so
    /// label values and provider ids can't drift apart.
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
            // The version trigger keys its host probe and loop guard on `id()`,
            // so drift would silently disable or cross-wire the trigger.
            assert_eq!(provider.id(), value.trim());
            assert_eq!(ContainerProvider::from_id(provider.id()), Some(provider));
        }
    }
}
