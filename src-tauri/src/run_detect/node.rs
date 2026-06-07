//! Node / JavaScript / TypeScript detector.

use super::port::detect_port;
use super::{
    exists, read_trimmed, DetectedConfig, DetectedRow, RowGroup, RunDetector,
    CONFIDENCE_LOCKFILE, CONFIDENCE_MANIFEST,
};
use std::path::Path;

pub struct NodeDetector;

/// (lockfile name, package-manager name) in priority order.
const LOCKFILES: &[(&str, &str)] = &[
    ("pnpm-lock.yaml", "pnpm"),
    ("yarn.lock", "yarn"),
    ("package-lock.json", "npm"),
    ("bun.lockb", "bun"),
];

impl RunDetector for NodeDetector {
    fn detect(&self, worktree: &Path) -> Option<DetectedConfig> {
        let raw = read_trimmed(worktree, "package.json")?;
        let pkg: serde_json::Value = serde_json::from_str(&raw).ok()?;

        // Package manager: `packageManager` field → lockfile → npm.
        let lockfile = LOCKFILES.iter().find(|(f, _)| exists(worktree, f));
        let pm = pkg
            .get("packageManager")
            .and_then(|v| v.as_str())
            .and_then(|s| s.split('@').next())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| lockfile.map(|(_, pm)| pm.to_string()))
            .unwrap_or_else(|| "npm".to_string());

        let confidence = if lockfile.is_some() {
            CONFIDENCE_LOCKFILE
        } else {
            CONFIDENCE_MANIFEST
        };

        let mut rows = Vec::new();

        // version: .nvmrc → .node-version → engines.node.
        if let Some((value, source)) = read_trimmed(worktree, ".nvmrc")
            .map(|v| (v, ".nvmrc"))
            .or_else(|| read_trimmed(worktree, ".node-version").map(|v| (v, ".node-version")))
            .or_else(|| {
                pkg.get("engines")
                    .and_then(|e| e.get("node"))
                    .and_then(|v| v.as_str())
                    .map(|v| (v.to_string(), "package.json · engines.node"))
            })
        {
            rows.push(DetectedRow::new(
                "version",
                RowGroup::Environment,
                "Node version",
                value,
                source,
            ));
        }

        // install — always available once a package manager is known.
        rows.push(DetectedRow::new(
            "install",
            RowGroup::Scripts,
            "Install",
            format!("{pm} install"),
            "package manager",
        ));

        let scripts = pkg.get("scripts");
        let script = |name: &str| -> Option<&str> {
            scripts.and_then(|s| s.get(name)).and_then(|v| v.as_str())
        };

        // dev: scripts.dev → scripts.start.
        let dev_value = if script("dev").is_some() {
            Some(("dev", format!("{pm} dev")))
        } else if script("start").is_some() {
            Some(("start", format!("{pm} start")))
        } else {
            None
        };
        if let Some((script_name, value)) = &dev_value {
            rows.push(DetectedRow::new(
                "dev",
                RowGroup::Scripts,
                "Dev",
                value.clone(),
                &format!("package.json · scripts.{script_name}"),
            ));
        }

        if script("build").is_some() {
            rows.push(DetectedRow::new(
                "build",
                RowGroup::Scripts,
                "Build",
                format!("{pm} build"),
                "package.json · scripts.build",
            ));
        }
        if script("test").is_some() {
            rows.push(DetectedRow::new(
                "test",
                RowGroup::Scripts,
                "Test",
                format!("{pm} test"),
                "package.json · scripts.test",
            ));
        }

        // port (optional): scan the resolved dev command + framework deps.
        let deps = dependency_names(&pkg);
        let dev_cmd = dev_value
            .as_ref()
            .and_then(|(name, _)| script(name))
            .unwrap_or("");
        if let Some((port, source)) = detect_port(dev_cmd, &deps) {
            rows.push(DetectedRow::new(
                "port",
                RowGroup::Server,
                "Port",
                port.to_string(),
                &source,
            ));
        }

        // env (optional): .env.local → .env.
        if let Some(file) = [".env.local", ".env"].iter().find(|f| exists(worktree, f)) {
            rows.push(DetectedRow::new(
                "env",
                RowGroup::Server,
                "Env file",
                *file,
                "present in worktree",
            ));
        }

        Some(DetectedConfig {
            ecosystem: "node".to_string(),
            confidence,
            rows,
        })
    }
}

/// Collect lowercased dependency names from `dependencies` and
/// `devDependencies` for framework/port heuristics.
fn dependency_names(pkg: &serde_json::Value) -> Vec<String> {
    let mut names = Vec::new();
    for field in ["dependencies", "devDependencies"] {
        if let Some(map) = pkg.get(field).and_then(|v| v.as_object()) {
            names.extend(map.keys().map(|k| k.to_lowercase()));
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{fixture, val};
    use super::*;

    fn detect(files: &[(&str, &str)]) -> Option<DetectedConfig> {
        let dir = fixture(files);
        NodeDetector.detect(dir.path())
    }

    #[test]
    fn no_package_json_is_none() {
        assert!(detect(&[("Cargo.toml", "")]).is_none());
    }

    #[test]
    fn lockfile_yields_high_confidence() {
        let cfg = detect(&[
            ("package.json", r#"{"scripts":{"dev":"vite"}}"#),
            ("pnpm-lock.yaml", ""),
        ])
        .unwrap();
        assert_eq!(cfg.ecosystem, "node");
        assert_eq!(cfg.confidence, CONFIDENCE_LOCKFILE);
    }

    #[test]
    fn manifest_only_is_medium_confidence() {
        let cfg = detect(&[("package.json", "{}")]).unwrap();
        assert_eq!(cfg.confidence, CONFIDENCE_MANIFEST);
    }

    #[test]
    fn package_manager_field_drives_install_command() {
        let cfg = detect(&[(
            "package.json",
            r#"{"packageManager":"pnpm@9.7.1","scripts":{"dev":"x"}}"#,
        )])
        .unwrap();
        assert_eq!(val(&cfg, "install"), "pnpm install");
    }

    #[test]
    fn lockfile_drives_pm_when_no_field() {
        let cfg = detect(&[
            ("package.json", "{}"),
            ("yarn.lock", ""),
        ])
        .unwrap();
        assert_eq!(val(&cfg, "install"), "yarn install");
    }

    #[test]
    fn defaults_to_npm() {
        let cfg = detect(&[("package.json", "{}")]).unwrap();
        assert_eq!(val(&cfg, "install"), "npm install");
    }

    #[test]
    fn scripts_become_rows() {
        let cfg = detect(&[(
            "package.json",
            r#"{"packageManager":"pnpm@9","scripts":{"dev":"next dev","build":"next build","test":"vitest"}}"#,
        )])
        .unwrap();
        assert_eq!(val(&cfg, "dev"), "pnpm dev");
        assert_eq!(val(&cfg, "build"), "pnpm build");
        assert_eq!(val(&cfg, "test"), "pnpm test");
    }

    #[test]
    fn dev_falls_back_to_start_script() {
        let cfg = detect(&[(
            "package.json",
            r#"{"scripts":{"start":"node server.js"}}"#,
        )])
        .unwrap();
        assert_eq!(val(&cfg, "dev"), "npm start");
    }

    #[test]
    fn missing_scripts_are_omitted() {
        let cfg = detect(&[("package.json", r#"{"scripts":{"dev":"x"}}"#)]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "build"));
        assert!(cfg.rows.iter().all(|r| r.id != "test"));
    }

    #[test]
    fn nvmrc_yields_version() {
        let cfg = detect(&[
            ("package.json", "{}"),
            (".nvmrc", "v22.4.0\n"),
        ])
        .unwrap();
        assert_eq!(val(&cfg, "version"), "v22.4.0");
    }

    #[test]
    fn engines_node_is_version_fallback() {
        let cfg = detect(&[("package.json", r#"{"engines":{"node":">=20"}}"#)]).unwrap();
        assert_eq!(val(&cfg, "version"), ">=20");
    }

    #[test]
    fn no_version_source_omits_version_row() {
        let cfg = detect(&[("package.json", "{}")]).unwrap();
        assert!(cfg.rows.iter().all(|r| r.id != "version"));
    }

    #[test]
    fn env_local_emits_env_row() {
        let cfg = detect(&[
            ("package.json", "{}"),
            (".env.local", "X=1"),
        ])
        .unwrap();
        assert_eq!(val(&cfg, "env"), ".env.local");
    }

    #[test]
    fn vite_dep_yields_default_port() {
        let cfg = detect(&[(
            "package.json",
            r#"{"scripts":{"dev":"vite"},"devDependencies":{"vite":"^5"}}"#,
        )])
        .unwrap();
        assert_eq!(val(&cfg, "port"), "5173");
    }

    #[test]
    fn explicit_port_flag_wins() {
        let cfg = detect(&[(
            "package.json",
            r#"{"scripts":{"dev":"vite --port 4000"},"devDependencies":{"vite":"^5"}}"#,
        )])
        .unwrap();
        assert_eq!(val(&cfg, "port"), "4000");
    }
}
