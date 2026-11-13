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
/// command. Services that only pull images come up before the native run
/// command, on every run, since setup runs once; a project that builds an
/// image runs as `docker compose up`, in the foreground so Stop ends it.
pub(super) fn apply(tree: &Tree, configs: &mut Vec<DetectedConfig>) {
    let Some(file) = FILES.iter().find(|f| tree.exists(f)) else {
        return;
    };
    let Some(builds) = tree
        .read_trimmed(file)
        .and_then(|text| builds_an_image(&text))
    else {
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

/// Whether some service in the compose file has a `build:` key. `None` when
/// the file is not YAML, as `docker compose` would refuse it too.
fn builds_an_image(text: &str) -> Option<bool> {
    let compose: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    let services = compose.get("services").and_then(|s| s.as_mapping());
    Some(services.is_some_and(|s| s.values().any(|service| service.get("build").is_some())))
}

fn set_run(config: &mut DetectedConfig, file: &str, builds: bool) {
    let native = config.rows.iter_mut().find(|r| r.id == "dev");
    match native {
        Some(row) if !builds => {
            row.value = format!("docker compose up -d --wait && {}", row.value);
            row.source = format!("{file} + {}", row.source);
        }
        Some(row) => {
            row.value = "docker compose up".to_string();
            row.source = file.to_string();
        }
        None => config.rows.push(DetectedRow::new(
            "dev",
            RowGroup::Scripts,
            "Run",
            "docker compose up",
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
    fn a_service_that_builds_makes_compose_the_run_command() {
        let configs = detect(&[ASTRO, PNPM_LOCK, ("compose.yaml", BUILDS)]);
        assert_eq!(val(&configs[0], "dev"), "docker compose up");
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
            assert_eq!(val(&configs[0], "dev"), "docker compose up", "{name}");
        }
    }

    #[test]
    fn a_compose_file_that_is_not_yaml_changes_nothing() {
        let configs = detect(&[ASTRO, PNPM_LOCK, ("compose.yaml", "services: [")]);
        assert_eq!(val(&configs[0], "dev"), "pnpm run dev");
    }
}
