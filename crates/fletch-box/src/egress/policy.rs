//! Who a box may reach: host patterns and ports.

use std::fmt;
use std::str::FromStr;

/// What a template entry is for, so the app can group the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// The agent CLIs' own APIs and sign-in hosts.
    Providers,
    /// Package registries.
    Registries,
    /// Toolchain installers and their download CDNs.
    Toolchains,
    /// Source hosting.
    SourceHosting,
    /// The guest's own package mirrors.
    OsPackages,
}

impl Category {
    /// The stable name the app keys on.
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Providers => "providers",
            Category::Registries => "registries",
            Category::Toolchains => "toolchains",
            Category::SourceHosting => "source_hosting",
            Category::OsPackages => "os_packages",
        }
    }
}

/// The template every box starts from, maintained here and grouped by what it
/// serves: the providers' full host sets, the package registries, the common
/// toolchain installers and CDNs, source hosting and the guest's apt mirrors.
/// The user's global list and a project's settings build on top of it.
///
/// What stays out on purpose: general-purpose upload sinks (cloud storage
/// buckets such as `storage.googleapis.com`, pastebins, tunnels), where a box
/// could put the code it can read; and telemetry ([`QUIET`]), which the CLIs
/// tolerate losing.
pub const TEMPLATE: &[(&str, Category)] = &[
    ("api.anthropic.com", Category::Providers),
    ("platform.claude.com", Category::Providers),
    ("*.anthropic.com", Category::Providers),
    ("*.claude.ai", Category::Providers),
    ("api.openai.com", Category::Providers),
    ("chatgpt.com", Category::Providers),
    ("auth.openai.com", Category::Providers),
    ("api2.cursor.sh", Category::Providers),
    ("*.cursor.sh", Category::Providers),
    ("api.cursor.com", Category::Providers),
    ("opencode.ai", Category::Providers),
    ("*.opencode.ai", Category::Providers),
    // The model providers the multi-provider CLIs (opencode, pi) talk to
    // directly, by the API keys they honour.
    ("generativelanguage.googleapis.com", Category::Providers),
    ("openrouter.ai", Category::Providers),
    ("api.groq.com", Category::Providers),
    ("api.x.ai", Category::Providers),
    ("api.deepseek.com", Category::Providers),
    ("api.mistral.ai", Category::Providers),
    ("registry.npmjs.org", Category::Registries),
    ("*.npmjs.org", Category::Registries),
    ("registry.yarnpkg.com", Category::Registries),
    ("repo.yarnpkg.com", Category::Registries),
    ("jsr.io", Category::Registries),
    ("pypi.org", Category::Registries),
    ("files.pythonhosted.org", Category::Registries),
    ("crates.io", Category::Registries),
    ("static.crates.io", Category::Registries),
    ("index.crates.io", Category::Registries),
    ("proxy.golang.org", Category::Registries),
    ("sum.golang.org", Category::Registries),
    ("repo.packagist.org", Category::Registries),
    ("rubygems.org", Category::Registries),
    ("index.rubygems.org", Category::Registries),
    ("repo1.maven.org", Category::Registries),
    ("repo.maven.apache.org", Category::Registries),
    ("plugins.gradle.org", Category::Registries),
    ("api.nuget.org", Category::Registries),
    ("static.rust-lang.org", Category::Toolchains),
    ("sh.rustup.rs", Category::Toolchains),
    // Cursor's CLI installer and its download CDN (the cursor tools disk).
    ("cursor.com", Category::Toolchains),
    ("downloads.cursor.com", Category::Toolchains),
    ("nodejs.org", Category::Toolchains),
    ("bun.sh", Category::Toolchains),
    ("deno.land", Category::Toolchains),
    ("dl.deno.land", Category::Toolchains),
    ("astral.sh", Category::Toolchains),
    ("bootstrap.pypa.io", Category::Toolchains),
    ("go.dev", Category::Toolchains),
    ("dl.google.com", Category::Toolchains),
    ("services.gradle.org", Category::Toolchains),
    ("downloads.gradle.org", Category::Toolchains),
    ("getcomposer.org", Category::Toolchains),
    ("cdn.playwright.dev", Category::Toolchains),
    (
        "playwright.download.prss.microsoft.com",
        Category::Toolchains,
    ),
    ("github.com", Category::SourceHosting),
    ("api.github.com", Category::SourceHosting),
    ("objects.githubusercontent.com", Category::SourceHosting),
    ("raw.githubusercontent.com", Category::SourceHosting),
    ("codeload.github.com", Category::SourceHosting),
    (
        "release-assets.githubusercontent.com",
        Category::SourceHosting,
    ),
    ("gitlab.com", Category::SourceHosting),
    ("bitbucket.org", Category::SourceHosting),
    ("deb.debian.org", Category::OsPackages),
    ("security.debian.org", Category::OsPackages),
    ("snapshot.debian.org", Category::OsPackages),
];

/// The template's patterns, in order. Kept for the callers that want the
/// plain list.
pub const DEFAULT_ALLOW: &[&str] = &{
    let mut out = [""; TEMPLATE.len()];
    let mut i = 0;
    while i < TEMPLATE.len() {
        out[i] = TEMPLATE[i].0;
        i += 1;
    }
    out
};

/// Hosts the agent CLIs and their tools reach for on their own — telemetry,
/// error reporting, feature flags — and that nothing needs: denied like any
/// host not allowed, but quietly. A denial of one is recorded, never surfaced
/// as something to act on. A user who allows one anyway is obeyed.
pub const QUIET: &[&str] = &[
    "*.datadoghq.com",
    "*.datadoghq.eu",
    "sentry.io",
    "*.sentry.io",
    "statsig.com",
    "*.statsig.com",
    "featuregates.org",
    "*.featuregates.org",
    "*.featureassets.org",
    "*.prodregistryv2.org",
    "api.segment.io",
    "*.posthog.com",
    "*.amplitude.com",
    "*.mixpanel.com",
    "api.honeycomb.io",
];

/// Whether a denial of `host` is noise ([`QUIET`]). `host` must be normalized.
pub fn is_quiet(host: &str) -> bool {
    QUIET.iter().any(|p| {
        p.parse::<HostPattern>()
            .map(|pattern| pattern.matches(host))
            .unwrap_or(false)
    })
}

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
            "generativelanguage.googleapis.com",
            "openrouter.ai",
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

    /// Every template and quiet pattern parses, none is listed twice, and the
    /// plain list is the template's patterns in order.
    #[test]
    fn the_template_and_quiet_list_are_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for (pattern, _) in TEMPLATE {
            assert!(pattern.parse::<HostPattern>().is_ok(), "{pattern}");
            assert!(seen.insert(*pattern), "listed twice: {pattern}");
        }
        for pattern in QUIET {
            assert!(pattern.parse::<HostPattern>().is_ok(), "{pattern}");
        }
        assert_eq!(
            DEFAULT_ALLOW,
            TEMPLATE
                .iter()
                .map(|(p, _)| *p)
                .collect::<Vec<_>>()
                .as_slice()
        );
    }

    /// The toolchains a box commonly installs are reached; telemetry is quiet
    /// and not allowed; a general-purpose upload sink is neither.
    #[test]
    fn toolchains_are_allowed_and_telemetry_is_quiet() {
        let policy = EgressPolicy::default();
        for host in [
            "static.rust-lang.org",
            "sh.rustup.rs",
            "bun.sh",
            "cdn.playwright.dev",
            "cursor.com",
            "downloads.cursor.com",
        ] {
            assert_eq!(policy.check(host, 443), Ok(()), "{host}");
        }
        for host in [
            "http-intake.logs.us5.datadoghq.com",
            "o1.ingest.sentry.io",
            "statsig.com",
        ] {
            assert!(is_quiet(host), "{host}");
            assert_eq!(policy.check(host, 443), Err(Denial::Host), "{host}");
        }
        assert!(!is_quiet("storage.googleapis.com"));
        assert_eq!(
            policy.check("storage.googleapis.com", 443),
            Err(Denial::Host)
        );
    }
}
