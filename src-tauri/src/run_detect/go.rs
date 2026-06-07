//! Go detector.

use super::{
    exists, read_trimmed, DetectedConfig, DetectedRow, RowGroup, RunDetector,
    CONFIDENCE_LOCKFILE, CONFIDENCE_MANIFEST,
};
use std::path::Path;

pub struct GoDetector;

impl RunDetector for GoDetector {
    fn detect(&self, worktree: &Path) -> Option<DetectedConfig> {
        let go_mod = read_trimmed(worktree, "go.mod")?;
        let confidence = if exists(worktree, "go.sum") {
            CONFIDENCE_LOCKFILE
        } else {
            CONFIDENCE_MANIFEST
        };

        let mut rows = Vec::new();

        // version: the `go X.Y` directive in go.mod.
        if let Some(version) = go_mod.lines().find_map(|l| {
            let l = l.trim();
            l.strip_prefix("go ")
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        }) {
            rows.push(DetectedRow::new(
                "version",
                RowGroup::Environment,
                "Go version",
                version,
                "go.mod · go directive",
            ));
        }

        rows.push(DetectedRow::new("install", RowGroup::Scripts, "Install", "go mod download", "convention"));
        rows.push(DetectedRow::new("dev", RowGroup::Scripts, "Run", "go run .", "convention"));
        rows.push(DetectedRow::new("build", RowGroup::Scripts, "Build", "go build ./...", "convention"));
        rows.push(DetectedRow::new("test", RowGroup::Scripts, "Test", "go test ./...", "convention"));

        Some(DetectedConfig {
            ecosystem: "go".to_string(),
            confidence,
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{fixture, val};
    use super::super::CONFIDENCE_LOCKFILE;
    use super::*;

    fn detect(files: &[(&str, &str)]) -> Option<DetectedConfig> {
        GoDetector.detect(fixture(files).path())
    }

    #[test]
    fn no_go_mod_is_none() {
        assert!(detect(&[("package.json", "{}")]).is_none());
    }

    #[test]
    fn go_sum_yields_high_confidence() {
        let cfg = detect(&[("go.mod", "module x\n"), ("go.sum", "")]).unwrap();
        assert_eq!(cfg.ecosystem, "go");
        assert_eq!(cfg.confidence, CONFIDENCE_LOCKFILE);
    }

    #[test]
    fn standard_go_commands() {
        let cfg = detect(&[("go.mod", "module x\n")]).unwrap();
        assert_eq!(val(&cfg, "install"), "go mod download");
        assert_eq!(val(&cfg, "dev"), "go run .");
        assert_eq!(val(&cfg, "build"), "go build ./...");
        assert_eq!(val(&cfg, "test"), "go test ./...");
    }

    #[test]
    fn go_directive_drives_version() {
        let cfg = detect(&[("go.mod", "module x\n\ngo 1.22\n")]).unwrap();
        assert_eq!(val(&cfg, "version"), "1.22");
    }

    #[test]
    fn no_go_directive_omits_version() {
        let cfg = detect(&[("go.mod", "module x\n")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "version"));
    }
}
