//! Reading the guest console for init's markers (`guest::MARK_*`).

use crate::guest;

/// The guest's SSH host key as init printed it: `FLETCH_HOSTKEY <type> <base64>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    pub key_type: String,
    pub base64: String,
}

impl HostKey {
    /// The `known_hosts` line for the box, under the ssh alias every
    /// `ssh_config` uses.
    pub fn known_hosts_line(&self) -> String {
        format!(
            "{} {} {}\n",
            guest::SSH_HOST_ALIAS,
            self.key_type,
            self.base64
        )
    }
}

/// What the console says about boot progress so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Booting {
        host_key: Option<HostKey>,
    },
    Ready(HostKey),
    /// `FLETCH_READY` without a preceding host key, or `FLETCH_INIT_ERROR`.
    Failed(String),
}

pub fn scan(console: &str) -> Progress {
    let mut host_key = None;
    for line in console.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix(guest::MARK_INIT_ERROR) {
            return Progress::Failed(format!("guest init failed: {}", rest.trim()));
        }
        if let Some(rest) = line.strip_prefix(guest::MARK_HOSTKEY) {
            let mut parts = rest.split_whitespace();
            if let (Some(key_type), Some(base64)) = (parts.next(), parts.next()) {
                host_key = Some(HostKey {
                    key_type: key_type.into(),
                    base64: base64.into(),
                });
            }
        }
        if line.trim() == guest::MARK_READY {
            return match host_key {
                Some(key) => Progress::Ready(key),
                None => Progress::Failed(format!(
                    "guest printed {} without a {} line",
                    guest::MARK_READY,
                    guest::MARK_HOSTKEY
                )),
            };
        }
    }
    Progress::Booting { host_key }
}

/// The last `max` bytes of the console, on a char boundary.
pub fn tail(console: &str, max: usize) -> &str {
    let mut start = console.len().saturating_sub(max);
    while !console.is_char_boundary(start) {
        start += 1;
    }
    &console[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_recognized_in_order() {
        assert_eq!(scan("booting\r\n"), Progress::Booting { host_key: None });
        let key = HostKey {
            key_type: "ssh-ed25519".into(),
            base64: "AAAAC3".into(),
        };
        assert_eq!(
            scan("x\nFLETCH_HOSTKEY ssh-ed25519 AAAAC3\n"),
            Progress::Booting {
                host_key: Some(key.clone())
            }
        );
        assert_eq!(
            scan("FLETCH_HOSTKEY ssh-ed25519 AAAAC3\r\nFLETCH_READY\r\nmore\n"),
            Progress::Ready(key.clone())
        );
        assert!(matches!(scan("FLETCH_READY\n"), Progress::Failed(_)));
        assert_eq!(
            scan("FLETCH_INIT_ERROR no data disk\n"),
            Progress::Failed("guest init failed: no data disk".into())
        );
        assert_eq!(key.known_hosts_line(), "box ssh-ed25519 AAAAC3\n");
    }

    #[test]
    fn tail_keeps_the_end() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 10), "ab");
        assert_eq!(tail("héllo", 4), "llo");
    }
}
