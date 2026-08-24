//! Container naming and the runtime-reserved exit-code messages: the pieces of
//! the launch/teardown path that carry no runtime coupling at all.
//!
//! Liveness lookups stay per-runtime — they shell out — and live next to their
//! runtime's CLI wrapper (`docker::engine::util`, `podman::engine::util`).

use std::sync::atomic::{AtomicU64, Ordering};

/// `fletch-<agent_id>-<8-char nonce>`. The nonce keeps respawns (view switch,
/// binary swap) from colliding with a predecessor container of the same agent
/// that `--rm` hasn't finished reaping yet; hashing in the pid keeps two
/// side-by-side Fletch instances apart even for a same-named agent.
pub(crate) fn container_name(agent_id: &str) -> String {
    // Container names must match [a-zA-Z0-9][a-zA-Z0-9_.-]* under both docker
    // and podman; the `fletch-` prefix fixes the first char, sanitize the rest.
    let sanitized: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("fletch-{sanitized}-{}", nonce())
}

/// 8 hex chars from (pid, monotonic counter): unique within a host across
/// concurrently running instances for the lifetime of any one container.
fn nonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::process::id().hash(&mut hasher);
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    let hex: String = format!("{:016x}", hasher.finish());
    hex[..8].to_string()
}

/// `Some(v)` only when `v` is present and non-blank — settings rows can hold
/// empty strings, which must fall back to the launch defaults.
pub(crate) fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// The runtime-specific wording [`describe_exit_code`] renders around. One
/// value per runtime, declared next to that runtime's engine.
pub(crate) struct ExitCopy {
    /// Display name of the runtime whose CLI reserved the code.
    pub runtime: &'static str,
    /// What reported a start failure: Docker has a daemon, Podman a machine.
    pub error_source: &'static str,
    /// One check the user can act on when the runtime couldn't start.
    pub remedy: &'static str,
    /// Settings key naming a user-supplied image, for runtimes that honor one.
    /// `None` drops the custom-image clause: a runtime with no override must
    /// not point the user at a setting it ignores.
    pub image_setting: Option<&'static str>,
}

/// User-readable meanings for a container CLI's reserved exit codes; other
/// codes are the contained agent's own and pass through unmapped. `run` relays
/// the agent's own exit status, so an agent that starts fine and later exits
/// 125/126/127 is indistinguishable from a launcher/image failure — the
/// messages name the likely runtime-layer cause but flag the agent-exit
/// possibility so they don't mislead when the container did launch.
///
/// Provider-neutral: the teardown plan carries only the container name, and the
/// sandbox image varies per provider, so these speak of "the agent binary"
/// rather than naming `claude`.
pub(crate) fn describe_exit_code(code: i32, copy: &ExitCopy) -> Option<String> {
    let ExitCopy {
        runtime,
        error_source,
        remedy,
        image_setting,
    } = copy;
    let msg = match code {
        125 => format!(
            "Exit 125: {runtime} could not start the sandbox container — {error_source} reported an error (or the agent itself exited 125). {remedy}"
        ),
        126 => {
            let mut msg = "Exit 126: the agent binary in the sandbox image is present but not runnable (or the agent itself exited 126).".to_string();
            if let Some(key) = image_setting {
                msg.push_str(&format!(" If you set a custom {key}, check its agent CLI."));
            }
            msg
        }
        127 => {
            let mut msg = "Exit 127: no agent binary on the sandbox image's PATH (or the agent itself exited 127).".to_string();
            if let Some(key) = image_setting {
                msg.push_str(&format!(
                    " A custom {key} must include the launching agent's CLI."
                ));
            }
            msg
        }
        _ => return None,
    };
    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A runtime with no image override must not mention one, and every message
    /// must still hedge about the agent's own exit status.
    #[test]
    fn exit_copy_without_an_image_setting_omits_the_override_clause() {
        let copy = ExitCopy {
            runtime: "Podman",
            error_source: "the machine",
            remedy: "Is the Podman machine running?",
            image_setting: None,
        };
        for code in [125, 126, 127] {
            let msg = describe_exit_code(code, &copy).unwrap();
            assert!(msg.contains("agent itself exited"), "must hedge: {msg}");
            assert!(!msg.contains("custom"), "no override to point at: {msg}");
        }
        assert!(describe_exit_code(125, &copy).unwrap().contains("Podman"));
        assert_eq!(describe_exit_code(1, &copy), None);
    }
}
