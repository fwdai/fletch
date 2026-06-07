//! Python detector.

use super::{
    exists, read_trimmed, DetectedConfig, DetectedRow, RowGroup, RunDetector,
    CONFIDENCE_LOCKFILE, CONFIDENCE_MANIFEST,
};
use std::path::Path;

pub struct PythonDetector;

impl RunDetector for PythonDetector {
    fn detect(&self, worktree: &Path) -> Option<DetectedConfig> {
        let has_pyproject = exists(worktree, "pyproject.toml");
        let has_requirements = exists(worktree, "requirements.txt");
        let has_pipfile = exists(worktree, "Pipfile");
        if !(has_pyproject || has_requirements || has_pipfile) {
            return None;
        }

        // install command + confidence keyed off the lockfile/manifest mix.
        let (install, lock_present) = if exists(worktree, "uv.lock") {
            ("uv sync", true)
        } else if exists(worktree, "poetry.lock") {
            ("poetry install", true)
        } else if has_pipfile {
            ("pipenv install", exists(worktree, "Pipfile.lock"))
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
            .filter_map(|f| read_trimmed(worktree, f))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        let has_dep = |name: &str| deps_text.contains(name);

        let mut rows = Vec::new();

        // version: .python-version → pyproject requires-python.
        if let Some((value, source)) = read_trimmed(worktree, ".python-version")
            .map(|v| (v, ".python-version"))
            .or_else(|| {
                read_trimmed(worktree, "pyproject.toml")
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

        rows.push(DetectedRow::new("install", RowGroup::Scripts, "Install", install, "convention"));

        // dev + port: Django (needs manage.py) → runserver:8000; Flask → 5000.
        if exists(worktree, "manage.py") && has_dep("django") {
            rows.push(DetectedRow::new("dev", RowGroup::Scripts, "Run", "python manage.py runserver", "Django"));
            rows.push(DetectedRow::new("port", RowGroup::Server, "Port", "8000", "default (Django)"));
        } else if has_dep("flask") {
            rows.push(DetectedRow::new("dev", RowGroup::Scripts, "Run", "flask run", "Flask"));
            rows.push(DetectedRow::new("port", RowGroup::Server, "Port", "5000", "default (Flask)"));
        }

        if has_dep("pytest") {
            rows.push(DetectedRow::new("test", RowGroup::Scripts, "Test", "pytest", "pytest dependency"));
        }

        Some(DetectedConfig {
            ecosystem: "python".to_string(),
            confidence,
            rows,
        })
    }
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
        let cfg = detect(&[
            ("requirements.txt", "Django==5.0\n"),
            ("manage.py", ""),
        ])
        .unwrap();
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
    fn plain_script_has_no_port() {
        let cfg = detect(&[("requirements.txt", "requests\n")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "port"));
    }
}
