//! Reading the guest console for init's markers (`guest::MARK_*`).

use crate::error::{Error, Result};
use crate::guest;

/// The guest's SSH host key as init printed it: `FLETCH_HOSTKEY <type> <base64>`;
/// or, for a guest with no console to read, as its `host_key.pub` holds it
/// ([`HostKey::from_public_key`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    pub key_type: String,
    pub base64: String,
}

/// The key types `ssh-keygen` makes a host key of.
const KEY_TYPES: &[&str] = &[
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "ssh-rsa",
];

impl HostKey {
    /// The key in one OpenSSH public key line, `<type> <base64>[ <comment>]`,
    /// the comment dropped. Strict, since the key becomes the box's
    /// `known_hosts`: a key type `ssh-keygen` makes, base64 as OpenSSH writes
    /// it (padded), and one line with no control character (a final newline
    /// aside).
    pub fn from_public_key(line: &str) -> Result<HostKey> {
        let line = line.strip_suffix('\n').unwrap_or(line);
        let invalid = || Error::Other(format!("not an OpenSSH public key line: {line:?}"));
        if line.chars().any(char::is_control) {
            return Err(invalid());
        }
        let mut fields = line.splitn(3, ' ');
        match (fields.next(), fields.next()) {
            (Some(key_type), Some(base64))
                if KEY_TYPES.contains(&key_type) && padded_base64(base64) =>
            {
                Ok(HostKey {
                    key_type: key_type.into(),
                    base64: base64.into(),
                })
            }
            _ => Err(invalid()),
        }
    }

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

/// Standard base64 with its padding: whole groups of four characters of
/// `A–Z a–z 0–9 + /`, the last ending in at most two `=`.
fn padded_base64(text: &str) -> bool {
    let body = text.trim_end_matches('=');
    !text.is_empty()
        && text.len() % 4 == 0
        && text.len() - body.len() <= 2
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
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
    fn a_public_key_line_gives_the_key_without_its_comment() {
        let key = |key_type: &str, base64: &str| HostKey {
            key_type: key_type.into(),
            base64: base64.into(),
        };
        let ed25519 = "AAAAC3NzaC1lZDI1NTE5AAAAIC0nN3Vj9q4X9a8c1nXkK8u0iL8bWv8bQ0o1Y4m5xY6p";
        for (line, expected) in [
            (
                format!("ssh-ed25519 {ed25519} fletch-box\n"),
                key("ssh-ed25519", ed25519),
            ),
            (
                format!("ssh-ed25519 {ed25519}"),
                key("ssh-ed25519", ed25519),
            ),
            (
                format!("ssh-ed25519 {ed25519} a comment with spaces"),
                key("ssh-ed25519", ed25519),
            ),
            (
                "ecdsa-sha2-nistp256 AAAA+/8= c".to_owned(),
                key("ecdsa-sha2-nistp256", "AAAA+/8="),
            ),
            ("ssh-rsa AAAAB3Nz".to_owned(), key("ssh-rsa", "AAAAB3Nz")),
        ] {
            assert_eq!(
                HostKey::from_public_key(&line).unwrap(),
                expected,
                "{line:?}"
            );
        }
        assert_eq!(
            HostKey::from_public_key(&format!("ssh-ed25519 {ed25519} x\n"))
                .unwrap()
                .known_hosts_line(),
            format!("box ssh-ed25519 {ed25519}\n")
        );
    }

    /// The key is written into `known_hosts`: nothing that could add a line, a
    /// marker or another host there gets through.
    #[test]
    fn anything_but_one_public_key_line_is_refused() {
        let ed25519 = "AAAAC3NzaC1lZDI1NTE5AAAAIC0nN3Vj9q4X9a8c1nXkK8u0iL8bWv8bQ0o1Y4m5xY6p";
        for line in [
            String::new(),
            "\n".into(),
            "ssh-ed25519".into(),
            format!("ssh-ed25519  {ed25519}"),
            format!(" ssh-ed25519 {ed25519}"),
            format!("ssh-ed25519\t{ed25519}"),
            format!("ssh-dss {ed25519}"),
            format!("@cert-authority ssh-ed25519 {ed25519}"),
            format!("box ssh-ed25519 {ed25519}"),
            format!("ssh-ed25519 {ed25519}\n\n"),
            format!("ssh-ed25519 {ed25519}\r\n"),
            format!("ssh-ed25519 {ed25519} x\nbox ssh-ed25519 {ed25519}"),
            format!("ssh-ed25519 {ed25519} x\0"),
            "ssh-ed25519 AAAAC3N".into(),
            "ssh-ed25519 AAAA====".into(),
            "ssh-ed25519 AA=A".into(),
            "ssh-ed25519 AAA-".into(),
            "ssh-ed25519 ====".into(),
        ] {
            assert!(HostKey::from_public_key(&line).is_err(), "{line:?}");
        }
    }

    #[test]
    fn tail_keeps_the_end() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 10), "ab");
        assert_eq!(tail("héllo", 4), "llo");
    }
}
