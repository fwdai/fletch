//! Ruby detector.

use super::{
    exists, read_trimmed, DetectedConfig, DetectedRow, RowGroup, RunDetector,
    CONFIDENCE_LOCKFILE, CONFIDENCE_MANIFEST,
};
use std::path::Path;

pub struct RubyDetector;

impl RunDetector for RubyDetector {
    fn detect(&self, worktree: &Path) -> Option<DetectedConfig> {
        let gemfile = read_trimmed(worktree, "Gemfile")?;
        let confidence = if exists(worktree, "Gemfile.lock") {
            CONFIDENCE_LOCKFILE
        } else {
            CONFIDENCE_MANIFEST
        };
        let gems = gemfile.to_lowercase();
        let has_gem = |name: &str| gems.contains(&format!("'{name}'")) || gems.contains(&format!("\"{name}\""));
        let is_rails = has_gem("rails");

        let mut rows = Vec::new();

        // version: .ruby-version → `ruby "x"` line in the Gemfile.
        if let Some(version) = read_trimmed(worktree, ".ruby-version").or_else(|| {
            gemfile.lines().find_map(|l| {
                let l = l.trim();
                l.strip_prefix("ruby ")
                    .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
                    .filter(|v| !v.is_empty())
            })
        }) {
            rows.push(DetectedRow::new(
                "version",
                RowGroup::Environment,
                "Ruby version",
                version,
                ".ruby-version",
            ));
        }

        rows.push(DetectedRow::new("install", RowGroup::Scripts, "Install", "bundle install", "convention"));

        if is_rails {
            rows.push(DetectedRow::new("dev", RowGroup::Scripts, "Run", "bin/rails server", "Rails"));
        }

        // test: rspec if present, else rake.
        let test_cmd = if has_gem("rspec") || has_gem("rspec-rails") {
            "bundle exec rspec"
        } else {
            "bundle exec rake test"
        };
        rows.push(DetectedRow::new("test", RowGroup::Scripts, "Test", test_cmd, "convention"));

        // port (optional): Rails conventional dev port.
        if is_rails {
            rows.push(DetectedRow::new("port", RowGroup::Server, "Port", "3000", "default (Rails)"));
        }

        Some(DetectedConfig {
            ecosystem: "ruby".to_string(),
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
        RubyDetector.detect(fixture(files).path())
    }

    #[test]
    fn no_gemfile_is_none() {
        assert!(detect(&[("package.json", "{}")]).is_none());
    }

    #[test]
    fn gemfile_lock_yields_high_confidence() {
        let cfg = detect(&[("Gemfile", "source 'x'\n"), ("Gemfile.lock", "")]).unwrap();
        assert_eq!(cfg.ecosystem, "ruby");
        assert_eq!(cfg.confidence, CONFIDENCE_LOCKFILE);
    }

    #[test]
    fn bundle_install_command() {
        let cfg = detect(&[("Gemfile", "source 'x'\n")]).unwrap();
        assert_eq!(val(&cfg, "install"), "bundle install");
    }

    #[test]
    fn rails_dev_and_port() {
        let cfg = detect(&[("Gemfile", "gem 'rails', '~> 7.0'\n")]).unwrap();
        assert_eq!(val(&cfg, "dev"), "bin/rails server");
        assert_eq!(val(&cfg, "port"), "3000");
    }

    #[test]
    fn non_rails_has_no_port() {
        let cfg = detect(&[("Gemfile", "gem 'sinatra'\n")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "port"));
    }

    #[test]
    fn rspec_test_command() {
        let cfg = detect(&[("Gemfile", "gem 'rspec'\n")]).unwrap();
        assert_eq!(val(&cfg, "test"), "bundle exec rspec");
    }

    #[test]
    fn default_test_is_rake() {
        let cfg = detect(&[("Gemfile", "gem 'rails'\n")]).unwrap();
        assert_eq!(val(&cfg, "test"), "bundle exec rake test");
    }

    #[test]
    fn ruby_version_file() {
        let cfg = detect(&[("Gemfile", "source 'x'\n"), (".ruby-version", "3.3.0\n")]).unwrap();
        assert_eq!(val(&cfg, "version"), "3.3.0");
    }
}
