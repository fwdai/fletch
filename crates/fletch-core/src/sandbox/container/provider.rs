//! Which providers Fletch can run inside a container, and the facts about each
//! one that every container runtime needs.

/// A provider Fletch can run inside a container sandbox — the single capability
/// gate, consulted instead of string-matching provider ids. Only the image,
/// config-dir mount and auth are provider-specific; the rest of a container is
/// provider-agnostic.
///
/// antigravity is absent deliberately: its CLI has no non-interactive credential
/// path (browser OAuth into the host keychain, no API-key fallback), so a fresh
/// container cannot authenticate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerProvider {
    Claude,
    Codex,
    Opencode,
    Pi,
    Cursor,
}

impl ContainerProvider {
    /// Every container-supported provider. The image GC derives the current
    /// expected images from this list — a variant missing here has its live
    /// image GC'd as stale.
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Opencode,
        Self::Pi,
        Self::Cursor,
    ];

    /// Map a provider id to its container support, or `None` when there is none
    /// — the launch gate turns `None` into a user-facing refusal.
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

    /// The provider id string — [`from_id`](Self::from_id)'s inverse, and the
    /// key for string-indexed state shared with the rest of the app.
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
            Self::Cursor => "cursor",
        }
    }

    /// The command name on the image's PATH, used as the in-image `agent_bin` —
    /// a host-resolved absolute path would be meaningless inside the container.
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
