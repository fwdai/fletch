//! PHP / Composer detector (Laravel, Symfony, plain Composer packages).

use super::{
    exists, read_trimmed, DetectedConfig, DetectedRow, RowGroup, RunDetector, CONFIDENCE_LOCKFILE,
    CONFIDENCE_MANIFEST,
};
use serde_json::Value;
use std::path::Path;

pub struct PhpDetector;

impl RunDetector for PhpDetector {
    fn detect(&self, checkout: &Path) -> Option<DetectedConfig> {
        let raw = read_trimmed(checkout, "composer.json")?;
        let manifest: Value = serde_json::from_str(&raw).ok()?;
        let confidence = if exists(checkout, "composer.lock") {
            CONFIDENCE_LOCKFILE
        } else {
            CONFIDENCE_MANIFEST
        };

        let has_script = |name: &str| {
            manifest
                .get("scripts")
                .and_then(|s| s.get(name))
                .is_some_and(|v| !v.is_null())
        };
        let requires = |name: &str| {
            ["require", "require-dev"]
                .iter()
                .any(|section| manifest.get(section).and_then(|s| s.get(name)).is_some())
        };
        // A Laravel *application* is marked by `artisan` at the root. Requiring
        // `laravel/framework` isn't enough: a package can depend on it (often
        // with Testbench) and has no `artisan` to run, so it falls through to
        // its declared test runner instead.
        let is_laravel = exists(checkout, "artisan");

        let mut rows = Vec::new();

        // version: the `require.php` constraint (e.g. "^8.2").
        if let Some(version) = manifest
            .get("require")
            .and_then(|r| r.get("php"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            rows.push(DetectedRow::new(
                "version",
                RowGroup::Environment,
                "PHP version",
                version,
                "composer.json · require.php",
            ));
        }

        rows.push(DetectedRow::new(
            "install",
            RowGroup::Scripts,
            "Install",
            "composer install",
            "composer.json",
        ));

        // dev: a `dev` Composer script (Laravel 11+ ships one that runs the
        // server, queue and Vite together) → `artisan serve`.
        if has_script("dev") {
            rows.push(DetectedRow::new(
                "dev",
                RowGroup::Scripts,
                "Dev",
                "composer dev",
                "composer.json · scripts.dev",
            ));
        } else if is_laravel {
            rows.push(DetectedRow::new(
                "dev",
                RowGroup::Scripts,
                "Dev",
                "php artisan serve",
                "Laravel",
            ));
        }

        // test: a `test` Composer script → `artisan test` → the declared test
        // runner. Nothing is invented for a package that declares none.
        let test = if has_script("test") {
            Some(("composer test", "composer.json · scripts.test"))
        } else if is_laravel {
            Some(("php artisan test", "Laravel"))
        } else if requires("pestphp/pest") {
            Some(("vendor/bin/pest", "composer.json · pestphp/pest"))
        } else if requires("phpunit/phpunit") {
            Some(("vendor/bin/phpunit", "composer.json · phpunit/phpunit"))
        } else {
            None
        };
        if let Some((cmd, source)) = test {
            rows.push(DetectedRow::new(
                "test",
                RowGroup::Scripts,
                "Test",
                cmd,
                source,
            ));
        }

        // lint: a `lint` Composer script, else the first declared linter, in
        // check-only mode so a verify run never rewrites files.
        let lint = if has_script("lint") {
            Some(("composer lint", "composer.json · scripts.lint"))
        } else if requires("laravel/pint") {
            Some(("vendor/bin/pint --test", "composer.json · laravel/pint"))
        } else if requires("friendsofphp/php-cs-fixer") {
            Some((
                "vendor/bin/php-cs-fixer fix --dry-run",
                "composer.json · friendsofphp/php-cs-fixer",
            ))
        } else if requires("phpstan/phpstan") || requires("larastan/larastan") {
            Some(("vendor/bin/phpstan analyse", "composer.json · phpstan"))
        } else {
            None
        };
        if let Some((cmd, source)) = lint {
            rows.push(DetectedRow::new(
                "lint",
                RowGroup::Scripts,
                "Lint",
                cmd,
                source,
            ));
        }

        // port (optional): `artisan serve`'s default.
        if is_laravel {
            rows.push(DetectedRow::new(
                "port",
                RowGroup::Server,
                "Port",
                "8000",
                "default (Laravel)",
            ));
        }

        Some(DetectedConfig {
            ecosystem: "php".to_string(),
            confidence,
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{fixture, row, val};
    use super::super::{CONFIDENCE_LOCKFILE, CONFIDENCE_MANIFEST};
    use super::*;

    fn detect(files: &[(&str, &str)]) -> Option<DetectedConfig> {
        PhpDetector.detect(fixture(files).path())
    }

    /// The `composer.json` a fresh Laravel 11+ app ships with (trimmed).
    const LARAVEL: &str = r#"{
        "require": { "php": "^8.2", "laravel/framework": "^11.0" },
        "require-dev": { "laravel/pint": "^1.13", "pestphp/pest": "^3.0" },
        "scripts": {
            "dev": ["Composer\\Config::disableProcessTimeout", "npx concurrently ..."],
            "test": ["@php artisan config:clear --ansi", "@php artisan test"]
        }
    }"#;

    #[test]
    fn no_composer_json_is_none() {
        assert!(detect(&[("package.json", "{}")]).is_none());
    }

    #[test]
    fn invalid_composer_json_is_none() {
        assert!(detect(&[("composer.json", "{ not json")]).is_none());
    }

    #[test]
    fn composer_lock_yields_high_confidence() {
        let cfg = detect(&[("composer.json", "{}"), ("composer.lock", "{}")]).unwrap();
        assert_eq!(cfg.ecosystem, "php");
        assert_eq!(cfg.confidence, CONFIDENCE_LOCKFILE);
    }

    #[test]
    fn manifest_only_yields_medium_confidence() {
        let cfg = detect(&[("composer.json", "{}")]).unwrap();
        assert_eq!(cfg.confidence, CONFIDENCE_MANIFEST);
    }

    #[test]
    fn composer_install_command() {
        let cfg = detect(&[("composer.json", "{}")]).unwrap();
        assert_eq!(val(&cfg, "install"), "composer install");
    }

    #[test]
    fn php_version_from_require() {
        let cfg = detect(&[("composer.json", r#"{"require":{"php":"^8.2"}}"#)]).unwrap();
        assert_eq!(val(&cfg, "version"), "^8.2");
        assert_eq!(
            row(&cfg, "version").unwrap().source,
            "composer.json · require.php"
        );
    }

    #[test]
    fn laravel_skeleton_uses_its_composer_scripts() {
        let cfg = detect(&[("composer.json", LARAVEL), ("artisan", "")]).unwrap();
        assert_eq!(val(&cfg, "test"), "composer test");
        assert_eq!(val(&cfg, "dev"), "composer dev");
        assert_eq!(val(&cfg, "lint"), "vendor/bin/pint --test");
        assert_eq!(val(&cfg, "port"), "8000");
    }

    #[test]
    fn laravel_without_scripts_falls_back_to_artisan() {
        let cfg = detect(&[
            (
                "composer.json",
                r#"{"require":{"laravel/framework":"^10.0"}}"#,
            ),
            ("artisan", ""),
        ])
        .unwrap();
        assert_eq!(val(&cfg, "test"), "php artisan test");
        assert_eq!(row(&cfg, "test").unwrap().source, "Laravel");
        assert_eq!(val(&cfg, "dev"), "php artisan serve");
    }

    #[test]
    fn artisan_file_alone_marks_laravel() {
        let cfg = detect(&[("composer.json", "{}"), ("artisan", "")]).unwrap();
        assert_eq!(val(&cfg, "test"), "php artisan test");
    }

    #[test]
    fn laravel_package_without_artisan_uses_its_runner() {
        // A package that requires the framework (e.g. for Testbench) has no
        // `artisan`: it must get its declared runner, not `php artisan test`.
        let cfg = detect(&[(
            "composer.json",
            r#"{"require":{"laravel/framework":"^11.0"},"require-dev":{"phpunit/phpunit":"^11.0"}}"#,
        )])
        .unwrap();
        assert_eq!(val(&cfg, "test"), "vendor/bin/phpunit");
        assert!(cfg.rows.iter().all(|r| r.id != "dev" && r.id != "port"));
    }

    #[test]
    fn pest_package_without_laravel() {
        let cfg = detect(&[(
            "composer.json",
            r#"{"require-dev":{"pestphp/pest":"^3.0","phpunit/phpunit":"^11.0"}}"#,
        )])
        .unwrap();
        assert_eq!(val(&cfg, "test"), "vendor/bin/pest");
    }

    #[test]
    fn phpunit_package_without_laravel() {
        let cfg = detect(&[(
            "composer.json",
            r#"{"require-dev":{"phpunit/phpunit":"^11.0"}}"#,
        )])
        .unwrap();
        assert_eq!(val(&cfg, "test"), "vendor/bin/phpunit");
    }

    #[test]
    fn no_test_runner_omits_test_row() {
        let cfg = detect(&[("composer.json", r#"{"require":{"php":"^8.2"}}"#)]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "test"));
    }

    #[test]
    fn non_laravel_has_no_dev_or_port() {
        let cfg = detect(&[("composer.json", "{}")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "dev" && r.id != "port"));
    }

    #[test]
    fn lint_script_wins_over_declared_linters() {
        let cfg = detect(&[(
            "composer.json",
            r#"{"scripts":{"lint":"pint --test"},"require-dev":{"laravel/pint":"^1"}}"#,
        )])
        .unwrap();
        assert_eq!(val(&cfg, "lint"), "composer lint");
    }

    #[test]
    fn php_cs_fixer_runs_dry() {
        let cfg = detect(&[(
            "composer.json",
            r#"{"require-dev":{"friendsofphp/php-cs-fixer":"^3"}}"#,
        )])
        .unwrap();
        assert_eq!(val(&cfg, "lint"), "vendor/bin/php-cs-fixer fix --dry-run");
    }

    #[test]
    fn larastan_yields_phpstan_lint() {
        let cfg = detect(&[(
            "composer.json",
            r#"{"require-dev":{"larastan/larastan":"^3"}}"#,
        )])
        .unwrap();
        assert_eq!(val(&cfg, "lint"), "vendor/bin/phpstan analyse");
    }

    #[test]
    fn no_linter_omits_lint_row() {
        let cfg = detect(&[("composer.json", "{}")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "lint"));
    }
}
