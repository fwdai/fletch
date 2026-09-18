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
        // inventing a command the project may not have installed). Detection is
        // section-aware (see `declares_package`), so a comment (`# ruff`), a
        // config table (`[tool.ruff]`), a script/task table
        // (`[tool.poe.tasks]`), a `[project]` `description`, or an unrelated
        // package (`ruffus`, `ruff-lsp`) never triggers it — otherwise
        // verification would run `ruff check .` and fail where Ruff isn't
        // installed.
        if declares_package(checkout, "ruff") {
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

/// Whether the project declares a runtime/dev dependency on package `name`
/// (lowercase), reading each manifest with awareness of its format.
///
/// A raw substring or even a bare per-line token isn't enough: a `[tool.ruff]`
/// config table, a `[tool.poe.tasks]` / `[tool.pdm.scripts]` entry like
/// `lint = "ruff check ."`, or a `[project]` `description = "ruff helpers"` all
/// mention Ruff without installing it, and running `ruff check .` would then
/// fail where the executable isn't on PATH. So detection is *contextual*:
///
/// * `requirements.txt` — the whole file is dependency context; the leading
///   PEP 508 package token of each non-comment, non-option line is the package.
/// * TOML (`pyproject.toml`, `Pipfile`) — a conservative line scanner tracks the
///   current `[section]` and only evaluates candidates inside known dependency
///   contexts (see [`toml_declares`]); everywhere else there are no candidates.
///
/// Package names match by whole token, so an unrelated package containing `name`
/// (`ruffus`, `sruff`, `ruff-lsp`) never counts.
fn declares_package(checkout: &Path, name: &str) -> bool {
    let in_requirements = read_trimmed(checkout, "requirements.txt")
        .is_some_and(|t| requirements_declare(&t.to_lowercase(), name));
    in_requirements
        || ["pyproject.toml", "Pipfile"].iter().any(|f| {
            read_trimmed(checkout, f).is_some_and(|t| toml_declares(&t.to_lowercase(), name))
        })
}

/// requirements.txt: the whole file is dependency context. Each non-blank line
/// that isn't a comment or an option flag (`-r other.txt`, `-e .`, `--hash …`)
/// begins with the package name.
fn requirements_declare(text: &str, name: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
            return false;
        }
        leading_package_token(line) == name
    })
}

/// A TOML section's relevance to dependency detection.
enum SectionKind {
    /// Every `key = "…"` line names a package (the key is the package):
    /// Poetry dependency tables, Pipfile `[packages]` / `[dev-packages]`.
    KeyDeps,
    /// Every key's *value* is an array of PEP 508 specs (the key is a group
    /// name): `[project.optional-dependencies]`, `[dependency-groups]`, pdm/uv
    /// dependency-group tables.
    ArrayTable,
    /// Mixed section where only specific keys open a dependency array
    /// (`[project]` → `dependencies`, `[tool.uv]` → `*-dependencies`).
    Mixed,
    /// No dependency candidates (config, scripts, metadata, …).
    None,
}

/// TOML dependency detection: a conservative line scanner (no TOML parser) that
/// tracks the current `[section]` and an open multi-line dependency array, and
/// only treats a line as a dependency candidate inside a known dependency
/// context. See [`SectionKind`].
fn toml_declares(text: &str, name: &str) -> bool {
    let mut section = String::new();
    // Inside an open `dependencies = [ … ]` array spanning multiple lines.
    let mut in_dep_array = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            // `[section]` / `[[array.of.tables]]`, tolerant of a trailing comment.
            let end = header.find(']').unwrap_or(header.len());
            section = header[..end].trim_start_matches('[').trim().to_string();
            in_dep_array = false;
            continue;
        }
        // Continuation lines of an open multi-line dependency array.
        if in_dep_array {
            if quoted_has_package(line, name) {
                return true;
            }
            if line.contains(']') {
                in_dep_array = false;
            }
            continue;
        }
        match section_kind(&section) {
            SectionKind::KeyDeps => {
                if leading_package_token(line) == name {
                    return true;
                }
            }
            SectionKind::ArrayTable => {
                let value = line.split_once('=').map_or(line, |(_, rhs)| rhs);
                if eval_array_value(value, name, &mut in_dep_array) {
                    return true;
                }
            }
            SectionKind::Mixed => {
                if let Some((key, rhs)) = line.split_once('=') {
                    if is_dep_key(&section, key.trim())
                        && eval_array_value(rhs, name, &mut in_dep_array)
                    {
                        return true;
                    }
                }
            }
            SectionKind::None => {}
        }
    }
    false
}

/// Classify a (lowercased) TOML section header for dependency detection.
fn section_kind(section: &str) -> SectionKind {
    match section {
        "project" | "tool.uv" => SectionKind::Mixed,
        "project.optional-dependencies"
        | "dependency-groups"
        | "tool.uv.dependency-groups"
        | "tool.pdm.dev-dependencies" => SectionKind::ArrayTable,
        "packages"
        | "dev-packages"
        | "tool.poetry.dependencies"
        | "tool.poetry.dev-dependencies" => SectionKind::KeyDeps,
        // `[tool.poetry.group.<name>.dependencies]` (incl. dev groups).
        s if s.starts_with("tool.poetry.group.") && s.ends_with(".dependencies") => {
            SectionKind::KeyDeps
        }
        _ => SectionKind::None,
    }
}

/// Whether a key opens a dependency array inside a [`SectionKind::Mixed`]
/// section — the only keys there whose value is a list of specs.
fn is_dep_key(section: &str, key: &str) -> bool {
    match section {
        "project" => key == "dependencies",
        "tool.uv" => matches!(
            key,
            "dev-dependencies" | "constraint-dependencies" | "override-dependencies"
        ),
        _ => false,
    }
}

/// Evaluate a dependency-array `value` (the RHS of `key = …`), which may be a
/// single-line array or the start of a multi-line one. Returns whether a quoted
/// spec's package token matches `name`; sets `in_dep_array` when the `[` is left
/// open past this line so continuation lines are scanned.
fn eval_array_value(value: &str, name: &str, in_dep_array: &mut bool) -> bool {
    if quoted_has_package(value, name) {
        return true;
    }
    if value.contains('[') && !value.contains(']') {
        *in_dep_array = true;
    }
    false
}

/// Whether any `"…"`-quoted spec in `s` has `name` as its leading package token.
fn quoted_has_package(s: &str, name: &str) -> bool {
    quoted_substrings(s)
        .iter()
        .any(|q| leading_package_token(q) == name)
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
    fn ruff_pep621_multiline_array_yields_lint_row() {
        let cfg = detect(&[(
            "pyproject.toml",
            "[project]\nname = \"x\"\ndependencies = [\n    \"requests\",\n    \"ruff>=0.4\",\n]\n",
        )])
        .unwrap();
        assert_eq!(val(&cfg, "lint"), "ruff check .");
    }

    #[test]
    fn ruff_poetry_group_dependency_yields_lint_row() {
        let cfg = detect(&[(
            "pyproject.toml",
            "[tool.poetry.group.dev.dependencies]\nruff = \"^0.4\"\n",
        )])
        .unwrap();
        assert_eq!(val(&cfg, "lint"), "ruff check .");
    }

    #[test]
    fn ruff_pipfile_dev_packages_yields_lint_row() {
        let cfg = detect(&[("Pipfile", "[dev-packages]\nruff = \"*\"\n")]).unwrap();
        assert_eq!(val(&cfg, "lint"), "ruff check .");
    }

    #[test]
    fn ruff_project_description_omits_lint_row() {
        // A `[project]` metadata key that merely mentions Ruff is not a dep.
        let cfg = detect(&[(
            "pyproject.toml",
            "[project]\nname = \"x\"\ndescription = \"ruff helpers for X\"\n",
        )])
        .unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "lint"));
    }

    #[test]
    fn ruff_task_table_omits_lint_row() {
        // A task/script table invoking Ruff doesn't install it.
        let cfg = detect(&[(
            "pyproject.toml",
            "[project]\nname = \"x\"\n\n[tool.poe.tasks]\nlint = \"ruff check .\"\n",
        )])
        .unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "lint"));
    }

    #[test]
    fn ruff_key_in_non_dependency_section_omits_lint_row() {
        // A bare `ruff = "..."` outside a dependency table is not a dependency.
        let cfg = detect(&[(
            "pyproject.toml",
            "[project]\nname = \"x\"\n\n[tool.something]\nruff = \"*\"\n",
        )])
        .unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "lint"));
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
