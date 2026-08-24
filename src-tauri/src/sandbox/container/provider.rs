//! Which providers Fletch can run inside a container, and the facts about each
//! one that every container runtime needs.

/// A provider Fletch can run inside a container sandbox. This is the single
/// capability gate the rest of the app consults instead of string-matching
/// `provider == "claude"`: [`supervisor::lifecycle::ensure_engine_supports_provider`]
/// refuses anything [`from_id`](ContainerProvider::from_id) doesn't recognize, and
/// the launch path (`sandbox::docker::engine`) branches on the variant for the
/// provider-specific image ([`images`](super::images)), config-dir mount, and
/// auth. Everything else about a container (workspace / RPC / object-store
/// mounts, naming, teardown) is provider-agnostic.
///
/// Seatbelt runs six providers; containers are being brought up one at a time as
/// each gets its image + config-mount + auth wired — claude, codex, opencode, pi,
/// and cursor so far. antigravity remains gated: its CLI (`agy`) has no
/// non-interactive credential path — auth is browser OAuth with its tokens in the
/// host keychain and no API-key env fallback (maintainer-confirmed), so a fresh
/// container cannot authenticate. See `ensure_engine_supports_provider`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerProvider {
    Claude,
    Codex,
    Opencode,
    Pi,
    Cursor,
}

impl ContainerProvider {
    /// Every container-supported provider. The image GC derives "the current
    /// expected images" from this list, so a variant missing here would make
    /// the GC treat that provider's live image as stale — when adding a
    /// variant, extend this list (the exhaustive `match` in
    /// [`images::image_spec`](super::images::image_spec) will already force you
    /// into that file).
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Opencode,
        Self::Pi,
        Self::Cursor,
    ];

    /// Map a provider id (as stamped on `AgentRecord.provider` / used by the
    /// frontend) to its container support, or `None` when the provider has no
    /// container support yet — the launch gate turns `None` into the
    /// user-facing "isn't available in container sandboxes yet" refusal.
    pub fn from_id(provider: &str) -> Option<Self> {
        match provider {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::Opencode),
            "pi" => Some(Self::Pi),
            "cursor" => Some(Self::Cursor),
            _ => None,
        }
    }

    /// The provider id string — [`from_id`](Self::from_id)'s inverse
    /// (round-trip enforced by a test in [`images`](super::images)). Used where
    /// a variant must key string-indexed state shared with the rest of the app,
    /// e.g. the host version probe (`agent::cached_provider_version`) and the
    /// persisted version-refresh loop guard (see `sandbox::docker::engine`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
            Self::Cursor => "cursor",
        }
    }

    /// The command name on the image's PATH — what this provider's npm package
    /// installs as its executable. Handed to `launch_agent` as the in-image
    /// `agent_bin` (a host-resolved absolute path would be meaningless inside
    /// the container). Matches the provider's `bin` field for both supported
    /// providers today, but named explicitly so it stays an image fact, not a
    /// coincidence.
    pub fn image_bin(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
            Self::Cursor => "cursor-agent",
        }
    }
}
