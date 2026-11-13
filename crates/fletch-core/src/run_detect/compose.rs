//! Docker Compose: the services a compose file at the root declares, started
//! by the run command.

use super::{DetectedConfig, DetectedRow, RowGroup, Tree, CONFIDENCE_MANIFEST};

/// The names `docker compose` looks for, in its order.
const FILES: &[&str] = &[
    "compose.yaml",
    "compose.yml",
    "docker-compose.yml",
    "docker-compose.yaml",
];

/// Make the compose file at the root, if any, part of every config's run
/// command. Its services come up before the native run command, on every
/// run, since setup runs once. Only when a service builds the project itself
/// does `docker compose up` replace the native command, in the foreground so
/// Stop ends it. What is built is rebuilt on each run (`--build`): an image
/// that copied the source in would otherwise keep running the old code.
pub(super) fn apply(tree: &Tree, configs: &mut Vec<DetectedConfig>) {
    let Some(file) = FILES.iter().find(|f| tree.exists(f)) else {
        return;
    };
    let Some(builds) = tree.read_trimmed(file).and_then(|text| builds(&text)) else {
        return;
    };
    if configs.is_empty() {
        configs.push(DetectedConfig {
            ecosystem: "compose".to_string(),
            confidence: CONFIDENCE_MANIFEST,
            rows: Vec::new(),
        });
    }
    for config in configs {
        set_run(config, file, builds);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Builds {
    /// Every service pulls its image.
    Nothing,
    /// Some service is built, none from the project's root: a dependency
    /// with an image of its own (a database with extensions, say).
    Dependencies,
    /// A service is built from the project's root: the app itself.
    TheProject,
}

/// What the compose file's services build. `None` when the file is not YAML,
/// as `docker compose` would refuse it too.
fn builds(text: &str) -> Option<Builds> {
    let compose: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    let services = compose.get("services").and_then(|s| s.as_mapping());
    let contexts: Vec<&str> = services
        .into_iter()
        .flat_map(|s| s.values())
        .filter_map(build_context)
        .collect();
    Some(if contexts.iter().any(|context| is_project_root(context)) {
        Builds::TheProject
    } else if contexts.is_empty() {
        Builds::Nothing
    } else {
        Builds::Dependencies
    })
}

/// The directory a service is built from: `build: <dir>`, or `build:
/// {context: <dir>}`, where compose takes the project's root for none.
fn build_context(service: &serde_yaml::Value) -> Option<&str> {
    let build = service.get("build")?;
    build
        .as_str()
        .or_else(|| build.get("context").and_then(|c| c.as_str()))
        .or(Some("."))
}

fn is_project_root(context: &str) -> bool {
    std::path::Path::new(context)
        .components()
        .all(|part| part == std::path::Component::CurDir)
}

fn set_run(config: &mut DetectedConfig, file: &str, builds: Builds) {
    let build = if builds == Builds::Nothing {
        ""
    } else {
        " --build"
    };
    let native = config.rows.iter_mut().find(|r| r.id == "dev");
    match native {
        Some(row) if builds != Builds::TheProject => {
            row.value = format!("docker compose up -d --wait{build} && {}", row.value);
            row.source = format!("{file} + {}", row.source);
        }
        Some(row) => {
            row.value = format!("docker compose up{build}");
            row.source = file.to_string();
        }
        None => config.rows.push(DetectedRow::new(
            "dev",
            RowGroup::Scripts,
            "Run",
            format!("docker compose up{build}"),
            file,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::super::node::NodeDetector;
    use super::super::test_support::{fixture, val};
    use super::super::{detect_all, RunDetector};
    use super::*;

    const ASTRO: (&str, &str) = ("package.json", r#"{"scripts":{"dev":"astro dev"}}"#);
    const PNPM_LOCK: (&str, &str) = ("pnpm-lock.yaml", "");

    const IMAGES_ONLY: &str = "services:
  db:
    image: postgres:16
  cache:
    image: redis:7
";

    const BUILDS: &str = "services:
  web:
    build: .
    ports:
      - \"3000:3000\"
  db:
    image: postgres:16
";

    fn detect(files: &[(&str, &str)]) -> Vec<DetectedConfig> {
        detect_all(fixture(files).path())
    }

    #[test]
    fn no_compose_file_leaves_detection_as_it_was() {
        let files = [ASTRO, PNPM_LOCK];
        let dir = fixture(&files);
        let node = NodeDetector.detect(&Tree::open(dir.path()).unwrap());
        assert_eq!(detect_all(dir.path()), vec![node.unwrap()]);
    }

    #[test]
    fn image_only_services_come_up_before_the_dev_command() {
        let configs = detect(&[ASTRO, PNPM_LOCK, ("compose.yaml", IMAGES_ONLY)]);
        assert_eq!(configs.len(), 1);
        assert_eq!(
            val(&configs[0], "dev"),
            "docker compose up -d --wait && pnpm run dev"
        );
        assert_eq!(val(&configs[0], "install"), "pnpm install");
    }

    #[test]
    fn a_service_built_from_the_root_makes_compose_the_run_command() {
        let configs = detect(&[ASTRO, PNPM_LOCK, ("compose.yaml", BUILDS)]);
        assert_eq!(val(&configs[0], "dev"), "docker compose up --build");
    }

    #[test]
    fn a_build_context_without_a_directory_is_the_root() {
        for build in [
            "build: ./",
            "build: {context: .}",
            "build: {dockerfile: Dockerfile.dev}",
        ] {
            let compose = format!("services:\n  web:\n    {build}\n");
            let configs = detect(&[ASTRO, PNPM_LOCK, ("compose.yaml", &compose)]);
            assert_eq!(
                val(&configs[0], "dev"),
                "docker compose up --build",
                "{build}"
            );
        }
    }

    #[test]
    fn a_dependency_that_is_built_leaves_the_native_command_running_the_app() {
        let compose = "services:
  db:
    build: ./docker/postgres
  search:
    build:
      context: services/search
";
        let configs = detect(&[ASTRO, PNPM_LOCK, ("compose.yaml", compose)]);
        assert_eq!(
            val(&configs[0], "dev"),
            "docker compose up -d --wait --build && pnpm run dev"
        );
    }

    #[test]
    fn a_compose_file_alone_is_run_with_compose() {
        let configs = detect(&[("compose.yaml", IMAGES_ONLY)]);
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].ecosystem, "compose");
        assert_eq!(val(&configs[0], "dev"), "docker compose up");
    }

    #[test]
    fn build_in_a_comment_or_a_string_is_not_a_build() {
        let compose = "services:
  db:
    image: postgres:16
    # build: ./db
    command: |
      build: not a key
    environment:
      NOTE: \"build: also not a key\"
";
        let configs = detect(&[ASTRO, PNPM_LOCK, ("compose.yaml", compose)]);
        assert_eq!(
            val(&configs[0], "dev"),
            "docker compose up -d --wait && pnpm run dev"
        );
    }

    #[test]
    fn every_compose_file_name_is_found() {
        for name in FILES {
            let configs = detect(&[(name, BUILDS)]);
            assert_eq!(
                val(&configs[0], "dev"),
                "docker compose up --build",
                "{name}"
            );
        }
    }

    #[test]
    fn a_compose_file_that_is_not_yaml_changes_nothing() {
        let configs = detect(&[ASTRO, PNPM_LOCK, ("compose.yaml", "services: [")]);
        assert_eq!(val(&configs[0], "dev"), "pnpm run dev");
    }
}
