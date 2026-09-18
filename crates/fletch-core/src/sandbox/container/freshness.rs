//! The image-freshness policy both container runtimes apply: how long a built
//! agent image may serve before a background rebuild, and whether a
//! host/in-image CLI version pair warrants one.
//!
//! Decisions only — every invocation they feed (the `image inspect`, the version
//! probe, the rebuild) lives in each runtime's own `image` module.

use std::time::Duration;

/// Dogma: **an agent image is never older than a week.** The images install
/// "latest at build time", so this bounds how long their contents stay frozen.
/// A stale image still serves the current launch and is rebuilt off-thread —
/// freshness is never a launch blocker. Deliberately not a setting.
pub(crate) const IMAGE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// TTL verdict against [`IMAGE_MAX_AGE`]. `Unknown` is its own state because
/// callers treat it as fresh: rebuilding on unparseable metadata would rebuild
/// on every resolution forever, the replacement parsing no better.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Freshness {
    Fresh,
    Stale,
    Unknown,
}

/// Classify a raw image creation timestamp (RFC3339, as both runtimes' `image
/// inspect` reports it). `now` is injected so tests use fixed instants.
pub(crate) fn classify_freshness(
    created_raw: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Freshness {
    let Ok(created) = chrono::DateTime::parse_from_rfc3339(created_raw.trim()) else {
        return Freshness::Unknown;
    };
    // A negative age (clock skew) compares below any positive TTL and lands on
    // Fresh — the right bias for bad clocks.
    let max_age = chrono::Duration::from_std(IMAGE_MAX_AGE).expect("TTL fits chrono::Duration");
    if now.signed_duration_since(created) > max_age {
        Freshness::Stale
    } else {
        Freshness::Fresh
    }
}

/// Pure core of the version-mismatch trigger: refresh only when both sides are
/// known, differ, and haven't already been attempted (rebuilding can't fix a
/// host pinned away from the registry's latest). Plain `!=` — "newer" isn't
/// computable across five vendors' version formats, and parity is the goal.
pub(crate) fn version_refresh_wanted(
    host: Option<&str>,
    container: Option<&str>,
    already_attempted: bool,
) -> bool {
    match (host, container) {
        (Some(h), Some(c)) => !already_attempted && h != c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_refresh_decision() {
        // Mismatch → refresh.
        assert!(version_refresh_wanted(
            Some("v2.0.1"),
            Some("v2.0.0"),
            false
        ));
        // No ordering, so host-older fires too.
        assert!(version_refresh_wanted(
            Some("v1.0.0"),
            Some("v2.0.0"),
            false
        ));
        // Match → fresh.
        assert!(!version_refresh_wanted(
            Some("v2.0.0"),
            Some("v2.0.0"),
            false
        ));
        // Either side unknown → inert.
        assert!(!version_refresh_wanted(None, Some("v2.0.0"), false));
        assert!(!version_refresh_wanted(Some("v2.0.0"), None, false));
        assert!(!version_refresh_wanted(None, None, false));
        // Already attempted → inert (a pinned host must not loop).
        assert!(!version_refresh_wanted(
            Some("v2.0.1"),
            Some("v2.0.0"),
            true
        ));
    }

    #[test]
    fn freshness_classification() {
        use chrono::{TimeZone, Utc};
        let now = Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap();

        // The runtimes' actual format: RFC3339 with nanoseconds and Z.
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
}
