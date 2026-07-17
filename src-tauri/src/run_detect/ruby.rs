//! Ruby detector.

use super::{
    exists, read_trimmed, DetectedConfig, DetectedRow, RowGroup, RunDetector, CONFIDENCE_LOCKFILE,
    CONFIDENCE_MANIFEST,
};
use std::path::Path;

pub struct RubyDetector;

impl RunDetector for RubyDetector {
    fn detect(&self, checkout: &Path) -> Option<DetectedConfig> {
        let gemfile = read_trimmed(checkout, "Gemfile")?;
        let confidence = if exists(checkout, "Gemfile.lock") {
            CONFIDENCE_LOCKFILE
        } else {
            CONFIDENCE_MANIFEST
        };
        let gems = gemfile.to_lowercase();
        let has_gem = |name: &str| {
            gems.contains(&format!("'{name}'")) || gems.contains(&format!("\"{name}\""))
        };
        let is_rails = has_gem("rails");

        let mut rows = Vec::new();

        // version: .ruby-version → `ruby "x"` line in the Gemfile. The
        // source reflects whichever branch actually supplied the value.
        if let Some((version, source)) = read_trimmed(checkout, ".ruby-version")
            .map(|v| (v, ".ruby-version"))
            .or_else(|| {
                gemfile
                    .lines()
                    .find_map(|l| {
                        let l = l.trim();
                        l.strip_prefix("ruby ")
                            .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
                            .filter(|v| !v.is_empty())
                    })
                    .map(|v| (v, "Gemfile · ruby"))
            })
        {
            rows.push(DetectedRow::new(
                "version",
                RowGroup::Environment,
                "Ruby version",
                version,
                source,
            ));
        }

        rows.push(DetectedRow::new(
            "install",
            RowGroup::Scripts,
            "Install",
            "bundle install",
            "convention",
        ));

        if is_rails {
            rows.push(DetectedRow::new(
                "dev",
                RowGroup::Scripts,
                "Run",
                "bin/rails server",
                "Rails",
            ));
        }

        // test: rspec if present, else rake.
        let test_cmd = if has_gem("rspec") || has_gem("rspec-rails") {
            "bundle exec rspec"
        } else {
            "bundle exec rake test"
        };
        rows.push(DetectedRow::new(
            "test",
            RowGroup::Scripts,
            "Test",
            test_cmd,
            "convention",
        ));

        // lint: only when RuboCop is a declared gem (conservative — no inventing
        // a command the project may not have). Its canonical invocation is
        // unambiguous.
        if has_gem("rubocop") {
            rows.push(DetectedRow::new(
                "lint",
                RowGroup::Scripts,
                "Lint",
                "bundle exec rubocop",
                "rubocop gem",
            ));
        }

        // port (optional): Rails conventional dev port.
        if is_rails {
            rows.push(DetectedRow::new(
                "port",
                RowGroup::Server,
                "Port",
                "3000",
                "default (Rails)",
            ));
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
    use super::super::test_support::{fixture, row, val};
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
    fn rubocop_gem_yields_lint_row() {
        let cfg = detect(&[("Gemfile", "gem 'rubocop'\n")]).unwrap();
        assert_eq!(val(&cfg, "lint"), "bundle exec rubocop");
    }

    #[test]
    fn no_rubocop_omits_lint_row() {
        let cfg = detect(&[("Gemfile", "gem 'rails'\n")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "lint"));
    }

    #[test]
    fn ruby_version_file() {
        let cfg = detect(&[("Gemfile", "source 'x'\n"), (".ruby-version", "3.3.0\n")]).unwrap();
        assert_eq!(val(&cfg, "version"), "3.3.0");
        assert_eq!(row(&cfg, "version").unwrap().source, ".ruby-version");
    }

    #[test]
    fn gemfile_ruby_directive_reports_gemfile_source() {
        // No .ruby-version: the value comes from the Gemfile `ruby`
        // directive, so the source must say so — not ".ruby-version".
        let cfg = detect(&[("Gemfile", "ruby \"3.2.2\"\nsource 'x'\n")]).unwrap();
        assert_eq!(val(&cfg, "version"), "3.2.2");
        assert_eq!(row(&cfg, "version").unwrap().source, "Gemfile · ruby");
    }
}
