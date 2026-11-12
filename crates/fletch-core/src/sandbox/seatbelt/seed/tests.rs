use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::capture::keep;
use super::start::start_checkout;
use super::*;
use crate::git::testing::{git, init_repo};
use crate::project_file::SeedSettings;
use crate::sandbox::seed::WORTH_INHERITING_KIB;

/// A file at `path` that makes the directories above it worth inheriting.
fn large(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, vec![7; (WORTH_INHERITING_KIB as usize + 64) * 1024]).unwrap();
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// A checkout named `repo` in `workspace`, whose base ignores `ignored` and
/// tracks `tracked`.
fn checkout(workspace: &Path, ignored: &[&str], tracked: &[&str]) -> PathBuf {
    let repo = workspace.join("repo");
    init_repo(&repo);
    write(&repo.join(".gitignore"), &ignored.join("\n"));
    for path in tracked {
        write(&repo.join(path), path);
    }
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    repo
}

/// A temporary directory, as its canonical path: what the clones need.
fn scratch() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}

/// A project of its own whose settings say `seed`.
fn project(seed: SeedSettings) -> String {
    let project_id = uuid::Uuid::new_v4().to_string();
    crate::project_file::set_seed_settings(&project_id, seed);
    project_id
}

fn at_rest(dir: &Path, agent_id: &str, project_id: &str) -> CheckoutsAtRest {
    let store = dir.join(SEEDS).join(project_id);
    CheckoutsAtRest {
        agent_id: agent_id.into(),
        project_id: project_id.into(),
        workspace: dir.join(agent_id),
        store,
        turns: turns_started(agent_id),
    }
}

async fn wait_for(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_capture_keeps_large_ignored_directories_where_they_were_and_nothing_else() {
    let (_dir, dir) = scratch();
    let workspace = dir.join("kansai");
    let ignored = [
        "node_modules/",
        "cache/",
        "small/",
        ".codegraph/",
        "uploads/",
        ".venv/",
        "*.log",
        ".env",
    ];
    let repo = checkout(
        &workspace,
        &ignored,
        &["README.md", "packages/app/main.js", "gen/main.js"],
    );
    large(&repo.join("node_modules/pkg/index.js"));
    large(&repo.join("node_modules/.cache/build"));
    large(&repo.join("packages/app/node_modules/dep/index.js"));
    large(&repo.join("gen/cache/blob"));
    large(&repo.join("uploads/photo"));
    large(&repo.join(".codegraph/index.db"));
    large(&repo.join("app.log"));
    write(&repo.join("small/cert.pem"), "cert");
    write(&repo.join(".env"), "SECRET=1");
    write(&repo.join("untracked.txt"), "wip");
    large(&repo.join(".venv/lib/site-packages/dep"));
    let script = repo.join(".venv/bin/pip");
    let interpreter = repo.join(".venv/bin/python");
    write(
        &script,
        &format!("#!{}\nimport pip\n", interpreter.display()),
    );
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    large(&workspace.join(".fletch-profile/skills/blob"));
    let tree = dir.join("candidate").join(TREE);
    let excluded = ["uploads", "node_modules/.cache", "gen"].map(String::from);

    let kept_kib = keep(&workspace, &tree, &excluded).await.unwrap();

    let kept = tree.join("repo");
    assert!(kept.join("node_modules/pkg/index.js").is_file());
    assert!(kept
        .join("packages/app/node_modules/dep/index.js")
        .is_file());
    for left_out in [
        ".git",
        "README.md",
        "packages/app/main.js",
        "untracked.txt",
        "app.log",
        ".env",
        "small",
        ".codegraph",
        "uploads",
        "node_modules/.cache",
        "gen",
        ".venv",
    ] {
        assert!(!kept.join(left_out).exists(), "{left_out} was kept");
    }
    let checkouts: Vec<_> = std::fs::read_dir(&tree)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(checkouts, ["repo"], "only checkouts are kept");
    assert!(kept_kib >= 2 * WORTH_INHERITING_KIB);
    assert!(repo.join("node_modules/pkg/index.js").is_file());
}

#[tokio::test]
async fn a_link_in_a_kept_directory_is_kept_as_the_link() {
    let (_dir, dir) = scratch();
    let workspace = dir.join("nara");
    let repo = checkout(&workspace, &["node_modules/"], &["README.md"]);
    large(&repo.join("node_modules/pkg/index.js"));
    std::fs::create_dir_all(repo.join("node_modules/.bin")).unwrap();
    symlink("../pkg/index.js", repo.join("node_modules/.bin/pkg")).unwrap();
    let outside = dir.join("outside");
    write(&outside.join("secret"), "secret");
    symlink(&outside, repo.join("node_modules/elsewhere")).unwrap();
    let tree = dir.join("candidate").join(TREE);

    keep(&workspace, &tree, &[]).await.unwrap();

    let kept = tree.join("repo/node_modules");
    assert_eq!(
        std::fs::read_link(kept.join(".bin/pkg")).unwrap(),
        Path::new("../pkg/index.js")
    );
    assert_eq!(std::fs::read_link(kept.join("elsewhere")).unwrap(), outside);
}

#[test]
fn only_the_end_of_a_turn_that_ran_offers_a_workspace() {
    let agent_id = "seatbelt-seed-between-turns";
    assert_eq!(end_turn(agent_id), None, "no turn ran");
    turn_started(agent_id);
    assert_eq!(turns_at_rest(agent_id), None, "a merge waits for the end");
    assert_eq!(end_turn(agent_id), Some(1));
    assert_eq!(end_turn(agent_id), None, "a second status ends no turn");
    assert_eq!(turns_at_rest(agent_id), Some(1));
}

#[test]
fn a_turn_that_did_not_start_after_all_leaves_the_workspace_at_rest() {
    let agent_id = "seatbelt-seed-turn-reverted";
    turn_started(agent_id);
    turn_start_reverted(agent_id);
    assert_eq!(
        turns_at_rest(agent_id),
        Some(1),
        "a merge is offered at once"
    );
    assert_eq!(end_turn(agent_id), None, "and no turn is left to end");
}

#[tokio::test]
async fn a_workspace_a_turn_started_in_while_it_was_taken_is_not_made_the_seed() {
    let (_dir, dir) = scratch();
    let agent_id = "seatbelt-seed-turn-started";
    let repo = checkout(&dir.join(agent_id), &["node_modules/"], &["README.md"]);
    large(&repo.join("node_modules/pkg/index.js"));
    let project_id = project(SeedSettings::default());

    let taken = at_rest(&dir, agent_id, &project_id);
    let candidate = dir.join("taken");
    std::fs::create_dir_all(&candidate).unwrap();
    turn_started(agent_id);
    assert!(taken.reduce(&candidate, &[]).await.is_err());

    let at_rest = at_rest(&dir, agent_id, &project_id);
    let candidate = dir.join("at-rest");
    std::fs::create_dir_all(&candidate).unwrap();
    assert!(at_rest.reduce(&candidate, &[]).await.unwrap() >= WORTH_INHERITING_KIB);
}

#[tokio::test]
async fn a_new_checkout_merges_the_seed_and_cleans_what_its_base_does_not_ignore() {
    let (_dir, dir) = scratch();
    let project_id = project(SeedSettings::default());
    let store = dir.join("store");
    let seed = store.join("seed").join(TREE).join("repo");
    write(&seed.join("node_modules/fresh/index.js"), "seed");
    write(&seed.join("node_modules/shared/index.js"), "seed");
    write(&seed.join("node_modules/shared/extra.js"), "seed");
    write(&seed.join("vendor/old.js"), "seed");
    let repo = checkout(&dir.join("osaka"), &["node_modules/"], &["README.md"]);
    write(&repo.join("node_modules/shared/index.js"), "base");

    start_checkout(store, project_id, &repo).await;

    let modules = repo.join("node_modules");
    assert_eq!(read(&modules.join("fresh/index.js")), "seed");
    assert_eq!(read(&modules.join("shared/index.js")), "base");
    assert_eq!(read(&modules.join("shared/extra.js")), "seed");
    assert!(
        !repo.join("vendor").exists(),
        "the base no longer ignores it"
    );
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn a_projects_first_workspace_seeds_the_next_and_a_merged_one_replaces_it() {
    let (_dir, dir) = scratch();
    let project_id = project(SeedSettings::default());
    let (first, second) = ("seatbelt-seed-first", "seatbelt-seed-second");
    let repo = checkout(&dir.join(first), &["node_modules/"], &["README.md"]);
    large(&repo.join("node_modules/pkg/index.js"));
    write(&repo.join("node_modules/pkg/built-by"), first);
    let store = dir.join(SEEDS).join(&project_id);
    let seed_built_by = || read(&store.join("seed/tree/repo/node_modules/pkg/built-by"));
    let no_candidate = || {
        std::fs::read_dir(&store).unwrap().flatten().all(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with("candidate-")
        })
    };

    seed::offer(at_rest(&dir, first, &project_id));
    wait_for(|| seed_built_by() == first).await;

    let next = checkout(&dir.join(second), &["node_modules/"], &["README.md"]);
    start_checkout(store.clone(), project_id.clone(), &next).await;
    assert_eq!(read(&next.join("node_modules/pkg/built-by")), first);
    assert!(next.join("node_modules/pkg/index.js").is_file());

    write(&next.join("node_modules/pkg/built-by"), second);
    seed::offer(at_rest(&dir, second, &project_id));
    assert!(no_candidate(), "a workspace not merged is not taken");
    assert_eq!(seed_built_by(), first);

    seed::want(&store, second).unwrap();
    seed::offer(at_rest(&dir, second, &project_id));
    wait_for(|| seed_built_by() == second).await;
    wait_for(|| !seed::is_wanted(&store, second)).await;
    assert_eq!(read(&repo.join("node_modules/pkg/built-by")), first);
}
