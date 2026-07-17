//! Python detector.

use super::{
    exists, read_trimmed, DetectedConfig, DetectedRow, RowGroup, RunDetector, CONFIDENCE_LOCKFILE,
    CONFIDENCE_MANIFEST,
};
use std::path::Path;

pub struct PythonDetector;

impl RunDetector for PythonDetector {
    fn detect(&self, checkout: &Path) -> Option<DetectedConfig> {
        let has_pyproject = exists(checkout, "pyproject.toml");
        let has_requirements = exists(checkout, "requirements.txt");
        let has_pipfile = exists(checkout, "Pipfile");
        if !(has_pyproject || has_requirements || has_pipfile) {
            return None;
        }

        // install command + confidence keyed off the lockfile/manifest mix.
        let (install, lock_present) = if exists(checkout, "uv.lock") {
            ("uv sync", true)
        } else if exists(checkout, "poetry.lock") {
            ("poetry install", true)
        } else if has_pipfile {
            ("pipenv install", exists(checkout, "Pipfile.lock"))
        } else if has_requirements {
            ("pip install -r requirements.txt", false)
        } else {
            ("pip install .", false)
        };
        let confidence = if lock_present {
            CONFIDENCE_LOCKFILE
        } else {
            CONFIDENCE_MANIFEST
        };

        // Combined lowercased dependency text for framework heuristics.
        let deps_text = ["requirements.txt", "pyproject.toml", "Pipfile"]
            .iter()
            .filter_map(|f| read_trimmed(checkout, f))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        let has_dep = |name: &str| deps_text.contains(name);

        let mut rows = Vec::new();

        // version: .python-version → pyproject requires-python.
        if let Some((value, source)) = read_trimmed(checkout, ".python-version")
            .map(|v| (v, ".python-version"))
            .or_else(|| {
                read_trimmed(checkout, "pyproject.toml")
                    .and_then(|s| requires_python(&s))
                    .map(|v| (v, "pyproject.toml · requires-python"))
            })
        {
            rows.push(DetectedRow::new(
                "version",
                RowGroup::Environment,
                "Python version",
                value,
                source,
            ));
        }

        rows.push(DetectedRow::new(
            "install",
            RowGroup::Scripts,
            "Install",
            install,
            "convention",
        ));

        // dev + port: Django (needs manage.py) → runserver:8000; Flask → 5000.
        if exists(checkout, "manage.py") && has_dep("django") {
            rows.push(DetectedRow::new(
                "dev",
                RowGroup::Scripts,
                "Run",
                "python manage.py runserver",
                "Django",
            ));
            rows.push(DetectedRow::new(
                "port",
                RowGroup::Server,
                "Port",
                "8000",
                "default (Django)",
            ));
        } else if has_dep("flask") {
            rows.push(DetectedRow::new(
                "dev",
                RowGroup::Scripts,
                "Run",
                "flask run",
                "Flask",
            ));
            rows.push(DetectedRow::new(
                "port",
                RowGroup::Server,
                "Port",
                "5000",
                "default (Flask)",
            ));
        }

        if has_dep("pytest") {
            rows.push(DetectedRow::new(
                "test",
                RowGroup::Scripts,
                "Test",
                "pytest",
                "pytest dependency",
            ));
        }

        // lint: only when Ruff is a declared *dependency* (conservative — no
        // inventing a command the project may not have installed). We match the
        // declared package *name*, not a raw substring, so a comment (`# ruff`),
        // a config table (`[tool.ruff]`), or an unrelated package (`ruffus`,
        // `ruff-lsp`) never triggers it — otherwise verification would run
        // `ruff check .` and fail where the executable isn't on PATH.
        if declares_package(&deps_text, "ruff") {
            rows.push(DetectedRow::new(
                "lint",
                RowGroup::Scripts,
                "Lint",
                "ruff check .",
                "ruff dependency",
            ));
        }

        Some(DetectedConfig {
            ecosystem: "python".to_string(),
            confidence,
            rows,
        })
    }
}

/// Whether `deps_text` (the combined, lowercased dependency manifests) declares
/// a dependency on package `name` (lowercase). Matches the leading PEP 508
/// package *token* of a requirement rather than a raw substring, across the
/// shapes Python manifests use:
///
/// * requirements.txt lines — `ruff`, `ruff==0.4`, `ruff[extra]`, `ruff>=0.1; …`;
/// * quoted specs in a PEP 621 / Poetry `dependencies` array — `"ruff>=0.1"`;
/// * the `ruff = "…"` key form of a Poetry / Pipfile dependency table.
///
/// Blank lines, comments (`# …`), and TOML section headers (`[tool.ruff]`) never
/// count, and an unrelated package whose name merely contains `name`
/// (`ruffus`, `sruff`, `ruff-lsp`) is rejected because the whole token must
/// match.
fn declares_package(deps_text: &str, name: &str) -> bool {
    deps_text.lines().any(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            return false;
        }
        // Candidates: the bare line (requirements.txt / TOML key form) plus every
        // quoted string on it (array elements / table values).
        std::iter::once(line)
            .chain(quoted_substrings(line))
            .any(|cand| leading_package_token(cand) == name)
    })
}

/// The leading PEP 508 package name of `spec`: the run of name characters
/// (`a-z0-9._-`) at its start, before any version / extras / marker punctuation
/// or a `key =`-style separator.
fn leading_package_token(spec: &str) -> &str {
    let spec = spec.trim();
    let end = spec
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
        .unwrap_or(spec.len());
    &spec[..end]
}

/// Every `"…"` / `'…'`-quoted substring in `line` (its inner text), so quoted
/// dependency specs are inspected without their surrounding array/table syntax.
fn quoted_substrings(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for quote in ['"', '\''] {
        let mut rest = line;
        while let Some(open) = rest.find(quote) {
            let after = &rest[open + 1..];
            match after.find(quote) {
                Some(close) => {
                    out.push(&after[..close]);
                    rest = &after[close + 1..];
                }
                None => break,
            }
        }
    }
    out
}

/// Extract `requires-python = "..."` from pyproject.toml text.
fn requires_python(toml: &str) -> Option<String> {
    toml.lines().find_map(|l| {
        let l = l.trim();
        l.strip_prefix("requires-python")
            .and_then(|r| r.trim_start().strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{fixture, val};
    use super::super::{CONFIDENCE_LOCKFILE, CONFIDENCE_MANIFEST};
    use super::*;

    fn detect(files: &[(&str, &str)]) -> Option<DetectedConfig> {
        PythonDetector.detect(fixture(files).path())
    }

    #[test]
    fn no_python_markers_is_none() {
        assert!(detect(&[("package.json", "{}")]).is_none());
    }

    #[test]
    fn requirements_txt_triggers_pip_install() {
        let cfg = detect(&[("requirements.txt", "flask\n")]).unwrap();
        assert_eq!(cfg.ecosystem, "python");
        assert_eq!(val(&cfg, "install"), "pip install -r requirements.txt");
        assert_eq!(cfg.confidence, CONFIDENCE_MANIFEST);
    }

    #[test]
    fn uv_lock_yields_uv_sync_and_high_confidence() {
        let cfg = detect(&[("pyproject.toml", "[project]\n"), ("uv.lock", "")]).unwrap();
        assert_eq!(val(&cfg, "install"), "uv sync");
        assert_eq!(cfg.confidence, CONFIDENCE_LOCKFILE);
    }

    #[test]
    fn poetry_lock_yields_poetry_install() {
        let cfg = detect(&[("pyproject.toml", "[tool.poetry]\n"), ("poetry.lock", "")]).unwrap();
        assert_eq!(val(&cfg, "install"), "poetry install");
    }

    #[test]
    fn pipfile_yields_pipenv_install() {
        let cfg = detect(&[("Pipfile", "[packages]\n")]).unwrap();
        assert_eq!(val(&cfg, "install"), "pipenv install");
    }

    #[test]
    fn python_version_file() {
        let cfg = detect(&[("requirements.txt", ""), (".python-version", "3.12.1\n")]).unwrap();
        assert_eq!(val(&cfg, "version"), "3.12.1");
    }

    #[test]
    fn requires_python_fallback() {
        let cfg = detect(&[(
            "pyproject.toml",
            "[project]\nrequires-python = \">=3.11\"\n",
        )])
        .unwrap();
        assert_eq!(val(&cfg, "version"), ">=3.11");
    }

    #[test]
    fn django_dev_and_port() {
        let cfg = detect(&[("requirements.txt", "Django==5.0\n"), ("manage.py", "")]).unwrap();
        assert_eq!(val(&cfg, "dev"), "python manage.py runserver");
        assert_eq!(val(&cfg, "port"), "8000");
    }

    #[test]
    fn flask_dev_and_port() {
        let cfg = detect(&[("requirements.txt", "flask\n")]).unwrap();
        assert_eq!(val(&cfg, "dev"), "flask run");
        assert_eq!(val(&cfg, "port"), "5000");
    }

    #[test]
    fn pytest_test_row() {
        let cfg = detect(&[("requirements.txt", "pytest\n")]).unwrap();
        assert_eq!(val(&cfg, "test"), "pytest");
    }

    #[test]
    fn ruff_dependency_yields_lint_row() {
        let cfg = detect(&[("requirements.txt", "ruff\n")]).unwrap();
        assert_eq!(val(&cfg, "lint"), "ruff check .");
    }

    #[test]
    fn ruff_pinned_spec_yields_lint_row() {
        let cfg = detect(&[("requirements.txt", "ruff==0.4.9\n")]).unwrap();
        assert_eq!(val(&cfg, "lint"), "ruff check .");
    }

    #[test]
    fn ruff_extras_spec_yields_lint_row() {
        let cfg = detect(&[("requirements.txt", "ruff[extra]>=0.1\n")]).unwrap();
        assert_eq!(val(&cfg, "lint"), "ruff check .");
    }

    #[test]
    fn ruff_pep621_array_spec_yields_lint_row() {
        let cfg = detect(&[(
            "pyproject.toml",
            "[project]\ndependencies = [\"requests\", \"ruff>=0.1\"]\n",
        )])
        .unwrap();
        assert_eq!(val(&cfg, "lint"), "ruff check .");
    }

    #[test]
    fn ruff_poetry_dependency_yields_lint_row() {
        let cfg = detect(&[(
            "pyproject.toml",
            "[tool.poetry.dependencies]\nruff = \"^0.4\"\n",
        )])
        .unwrap();
        assert_eq!(val(&cfg, "lint"), "ruff check .");
    }

    #[test]
    fn ruff_config_section_alone_omits_lint_row() {
        // `[tool.ruff]` configures Ruff but doesn't install it; verification
        // must not assume the executable is present.
        let cfg = detect(&[(
            "pyproject.toml",
            "[project]\nname = \"x\"\n\n[tool.ruff]\nline-length = 100\n",
        )])
        .unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "lint"));
    }

    #[test]
    fn ruff_comment_mention_omits_lint_row() {
        // A commented-out / aspirational mention is not a declared dependency.
        let cfg = detect(&[("requirements.txt", "requests\n# TODO: add ruff later\n")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "lint"));
    }

    #[test]
    fn ruff_substring_package_omits_lint_row() {
        // `ruffus` / `sruff` merely contain the substring; the whole package
        // token must match, so neither yields a Ruff lint row.
        let cfg = detect(&[("requirements.txt", "ruffus==2.8\nsruff\n")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "lint"));
    }

    #[test]
    fn no_linter_dependency_omits_lint_row() {
        let cfg = detect(&[("requirements.txt", "requests\n")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "lint"));
    }

    #[test]
    fn plain_script_has_no_port() {
        let cfg = detect(&[("requirements.txt", "requests\n")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "port"));
    }
}
