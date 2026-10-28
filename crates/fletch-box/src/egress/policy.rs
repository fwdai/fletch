//! Who a box may reach: host patterns and ports.

use std::fmt;
use std::str::FromStr;

/// The hosts every box may reach (docs/boxes-spec.md §8): the providers' full
/// host sets and the package registries. Telemetry hosts are deliberately
/// absent; the CLIs tolerate their denial.
pub const DEFAULT_ALLOW: &[&str] = &[
    "api.anthropic.com",
    "platform.claude.com",
    "*.anthropic.com",
    "*.claude.ai",
    "api.openai.com",
    "chatgpt.com",
    "auth.openai.com",
    "api2.cursor.sh",
    "*.cursor.sh",
    "api.cursor.com",
    "opencode.ai",
    "*.opencode.ai",
    "registry.npmjs.org",
    "*.npmjs.org",
    "pypi.org",
    "files.pythonhosted.org",
    "crates.io",
    "static.crates.io",
    "index.crates.io",
    "github.com",
    "api.github.com",
    "objects.githubusercontent.com",
    "raw.githubusercontent.com",
    "codeload.github.com",
    "deb.debian.org",
    "security.debian.org",
    "proxy.golang.org",
    "sum.golang.org",
    "repo.packagist.org",
    "rubygems.org",
];

pub const DEFAULT_PORTS: &[u16] = &[80, 443];

/// An exact host name, or `*.domain` for every name below a domain (not the
/// domain itself; list both for both).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPattern {
    Exact(String),
    /// Stored as `.domain`.
    Suffix(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid host pattern {0:?}: use a host name or *.domain")]
pub struct PatternError(pub String);

impl FromStr for HostPattern {
    type Err = PatternError;

    fn from_str(pattern: &str) -> Result<HostPattern, PatternError> {
        let invalid = || PatternError(pattern.into());
        match pattern.strip_prefix("*.") {
            Some(domain) => Ok(HostPattern::Suffix(format!(
                ".{}",
                normalize_host(domain).ok_or_else(invalid)?
            ))),
            None => Ok(HostPattern::Exact(
                normalize_host(pattern).ok_or_else(invalid)?,
            )),
        }
    }
}

impl fmt::Display for HostPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostPattern::Exact(name) => f.write_str(name),
            HostPattern::Suffix(suffix) => write!(f, "*{suffix}"),
        }
    }
}

impl HostPattern {
    /// `host` must already be normalized ([`normalize_host`]).
    pub fn matches(&self, host: &str) -> bool {
        match self {
            HostPattern::Exact(name) => host == name,
            HostPattern::Suffix(suffix) => host.ends_with(suffix.as_str()),
        }
    }
}

/// Lowercase, no trailing dot; `None` if it is not a plausible DNS name.
pub fn normalize_host(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let label_ok = |label: &str| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    (!host.is_empty() && host.len() <= 253 && host.split('.').all(label_ok)).then_some(host)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressPolicy {
    pub allow: Vec<HostPattern>,
    pub ports: Vec<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    Host,
    Port,
}

impl fmt::Display for Denial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Denial::Host => "host not allowed",
            Denial::Port => "port not allowed",
        })
    }
}

impl Default for EgressPolicy {
    /// [`DEFAULT_ALLOW`] on [`DEFAULT_PORTS`].
    fn default() -> EgressPolicy {
        EgressPolicy::new(DEFAULT_ALLOW.iter().copied()).expect("DEFAULT_ALLOW parses")
    }
}

impl EgressPolicy {
    /// Only these patterns, on the default ports.
    pub fn new<S: AsRef<str>>(
        patterns: impl IntoIterator<Item = S>,
    ) -> Result<EgressPolicy, PatternError> {
        Ok(EgressPolicy {
            allow: patterns
                .into_iter()
                .map(|p| p.as_ref().parse())
                .collect::<Result<_, _>>()?,
            ports: DEFAULT_PORTS.to_vec(),
        })
    }

    /// The defaults plus a project's additions.
    pub fn with_extra<S: AsRef<str>>(
        patterns: impl IntoIterator<Item = S>,
    ) -> Result<EgressPolicy, PatternError> {
        let mut policy = EgressPolicy::default();
        policy.allow.extend(
            patterns
                .into_iter()
                .map(|p| p.as_ref().parse())
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(policy)
    }

    pub fn check(&self, host: &str, port: u16) -> Result<(), Denial> {
        if !self.ports.contains(&port) {
            return Err(Denial::Port);
        }
        if self.allow.iter().any(|p| p.matches(host)) {
            Ok(())
        } else {
            Err(Denial::Host)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_exact_names_and_subdomains() {
        let policy = EgressPolicy::new(["Example.com.", "*.pypi.org"]).unwrap();
        assert_eq!(policy.check("example.com", 443), Ok(()));
        assert_eq!(policy.check("files.pypi.org", 443), Ok(()));
        assert_eq!(policy.check("a.b.pypi.org", 80), Ok(()));
        for host in [
            "pypi.org",
            "www.example.com",
            "evilpypi.org",
            "example.com.evil",
        ] {
            assert_eq!(policy.check(host, 443), Err(Denial::Host), "{host}");
        }
        assert_eq!(policy.check("example.com", 22), Err(Denial::Port));
        assert_eq!(
            "*.pypi.org".parse::<HostPattern>().unwrap().to_string(),
            "*.pypi.org"
        );
    }

    #[test]
    fn bad_patterns_are_rejected() {
        for bad in ["", "*", "*.", "a b", "http://x", "a..b", "x/y"] {
            assert!(bad.parse::<HostPattern>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn defaults_cover_the_providers() {
        let policy = EgressPolicy::default();
        for host in [
            "api.anthropic.com",
            "platform.claude.com",
            "registry.npmjs.org",
            "api2.cursor.sh",
            "github.com",
        ] {
            assert_eq!(policy.check(host, 443), Ok(()), "{host}");
        }
        assert_eq!(
            policy.check("http-intake.logs.us5.datadoghq.com", 443),
            Err(Denial::Host)
        );
        let extra = EgressPolicy::with_extra(["example.com"]).unwrap();
        assert_eq!(extra.check("example.com", 443), Ok(()));
        assert_eq!(extra.allow.len(), DEFAULT_ALLOW.len() + 1);
    }
}
