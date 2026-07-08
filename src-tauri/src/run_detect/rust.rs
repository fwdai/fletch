//! Rust / Cargo detector.

use super::{
    exists, read_trimmed, DetectedConfig, DetectedRow, RowGroup, RunDetector,
    CONFIDENCE_LOCKFILE, CONFIDENCE_MANIFEST,
};
use std::path::Path;

pub struct RustDetector;

impl RunDetector for RustDetector {
    fn detect(&self, checkout: &Path) -> Option<DetectedConfig> {
        if !exists(checkout, "Cargo.toml") {
            return None;
        }
        let confidence = if exists(checkout, "Cargo.lock") {
            CONFIDENCE_LOCKFILE
        } else {
            CONFIDENCE_MANIFEST
        };

        let mut rows = Vec::new();

        // version: rust-toolchain.toml `channel`, or legacy bare
        // rust-toolchain file.
        if let Some(channel) = read_trimmed(checkout, "rust-toolchain.toml")
            .and_then(|s| toml_string_value(&s, "channel"))
        {
            rows.push(DetectedRow::new(
                "version",
                RowGroup::Environment,
                "Toolchain",
                channel,
                "rust-toolchain.toml · channel",
            ));
        } else if let Some(channel) = read_trimmed(checkout, "rust-toolchain") {
            rows.push(DetectedRow::new(
                "version",
                RowGroup::Environment,
                "Toolchain",
                channel,
                "rust-toolchain",
            ));
        }

        rows.push(DetectedRow::new("install", RowGroup::Scripts, "Install", "cargo fetch", "convention"));
        rows.push(DetectedRow::new("dev", RowGroup::Scripts, "Run", "cargo run", "convention"));
        rows.push(DetectedRow::new("build", RowGroup::Scripts, "Build", "cargo build", "convention"));
        rows.push(DetectedRow::new("test", RowGroup::Scripts, "Test", "cargo test", "convention"));

        Some(DetectedConfig {
            ecosystem: "rust".to_string(),
            confidence,
            rows,
        })
    }
}

/// Pull a `key = "value"` string out of simple TOML text. Good enough for
/// reading single scalar fields without a full TOML parser.
fn toml_string_value(toml: &str, key: &str) -> Option<String> {
    for line in toml.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                return Some(rest.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{fixture, val};
    use super::super::CONFIDENCE_LOCKFILE;
    use super::*;

    fn detect(files: &[(&str, &str)]) -> Option<DetectedConfig> {
        RustDetector.detect(fixture(files).path())
    }

    #[test]
    fn no_cargo_toml_is_none() {
        assert!(detect(&[("package.json", "{}")]).is_none());
    }

    #[test]
    fn cargo_lock_yields_high_confidence() {
        let cfg = detect(&[("Cargo.toml", "[package]\nname=\"x\""), ("Cargo.lock", "")]).unwrap();
        assert_eq!(cfg.ecosystem, "rust");
        assert_eq!(cfg.confidence, CONFIDENCE_LOCKFILE);
    }

    #[test]
    fn standard_cargo_commands() {
        let cfg = detect(&[("Cargo.toml", "[package]\nname=\"x\"")]).unwrap();
        assert_eq!(val(&cfg, "install"), "cargo fetch");
        assert_eq!(val(&cfg, "dev"), "cargo run");
        assert_eq!(val(&cfg, "build"), "cargo build");
        assert_eq!(val(&cfg, "test"), "cargo test");
    }

    #[test]
    fn toolchain_file_drives_version() {
        let cfg = detect(&[
            ("Cargo.toml", "[package]"),
            ("rust-toolchain.toml", "[toolchain]\nchannel = \"1.75.0\"\n"),
        ])
        .unwrap();
        assert_eq!(val(&cfg, "version"), "1.75.0");
    }

    #[test]
    fn no_toolchain_omits_version() {
        let cfg = detect(&[("Cargo.toml", "[package]")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "version"));
    }

    #[test]
    fn no_port_row() {
        let cfg = detect(&[("Cargo.toml", "[package]")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "port"));
    }
}
