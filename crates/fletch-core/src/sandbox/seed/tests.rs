use std::path::{Path, PathBuf};

use super::candidate::Candidate;
use super::request::{standing_request, withdraw, Request};
use super::store::{publish, seed_in_force, Made, CANDIDATE_PREFIX, MADE, SEED};
use super::{forget, is_wanted, want, wanted, AtRest, WORTH_INHERITING_KIB};
use crate::error::Result;
use crate::project_file::SeedSettings;

const PROJECT_ID: &str = "seed-core-tests";
const AGENT_ID: &str = "kansai";

/// A workspace at rest whose candidate already holds what was taken of it,
/// and keeps something worth inheriting.
struct Workspace {
    store: PathBuf,
    may_be_first: bool,
    usable: bool,
}

impl AtRest for Workspace {
    fn agent_id(&self) -> &str {
        AGENT_ID
    }

    fn project_id(&self) -> &str {
        PROJECT_ID
    }

    fn store(&self) -> &Path {
        &self.store
    }

    fn may_be_first(&self) -> bool {
        self.may_be_first
    }

    fn usable(&self, _seed: &Path) -> bool {
        self.usable
    }

    async fn reduce(&self, _candidate: &Path, _excluded: &[String]) -> Result<u64> {
        Ok(WORTH_INHERITING_KIB)
    }
}

fn made(without: &[&str], for_request: Option<Request>) -> Made {
    Made {
        without: without.iter().map(|path| path.to_string()).collect(),
        for_request,
    }
}

fn project_with_seed(dir: &Path, held: &[u8], made: &Made) -> PathBuf {
    let project = dir.join("project");
    let seed = project.join(SEED);
    std::fs::create_dir_all(&seed).unwrap();
    std::fs::write(seed.join("held"), held).unwrap();
    std::fs::write(seed.join(MADE), serde_json::to_vec(made).unwrap()).unwrap();
    project
}

fn candidate_dir(project: &Path, held: &str) -> PathBuf {
    let candidate = project.join(format!("{CANDIDATE_PREFIX}test"));
    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::write(candidate.join("held"), held).unwrap();
    candidate
}

fn candidate(project: &Path, may_be_first: bool, request: Option<Request>) -> Candidate<Workspace> {
    Candidate {
        at_rest: Workspace {
            store: project.to_path_buf(),
            may_be_first,
            usable: true,
        },
        dir: project.join(format!("{CANDIDATE_PREFIX}test")),
        request,
    }
}

fn new_request(store: &Path) -> Request {
    want(store, AGENT_ID).unwrap();
    standing_request(store, AGENT_ID).unwrap()
}

fn seed_held(project: &Path) -> Option<String> {
    std::fs::read_to_string(project.join(SEED).join("held")).ok()
}

fn in_force(project: &Path, settings: &SeedSettings) -> bool {
    seed_in_force(project, settings, |_| true).is_some()
}

fn excluding(paths: &[&str]) -> SeedSettings {
    SeedSettings {
        enabled: true,
        exclude: paths.iter().map(|path| path.to_string()).collect(),
    }
}

fn seeds_off() -> SeedSettings {
    SeedSettings {
        enabled: false,
        exclude: Vec::new(),
    }
}

#[test]
fn a_seed_made_before_a_path_was_excluded_is_removed_not_started_from() {
    let dir = tempfile::tempdir().unwrap();
    let project = project_with_seed(dir.path(), b"seed", &made(&["uploads"], None));

    assert!(in_force(&project, &excluding(&[])));
    assert!(in_force(&project, &excluding(&["uploads"])));

    assert!(!in_force(&project, &excluding(&["uploads", "certs"])));
    assert!(!project.join(SEED).exists());
}

#[test]
fn a_project_that_turned_seeds_off_loses_the_one_it_had() {
    let dir = tempfile::tempdir().unwrap();
    let project = project_with_seed(dir.path(), b"seed", &made(&[], None));

    assert!(!in_force(&project, &seeds_off()));
    assert!(!project.join(SEED).exists());
}

#[test]
fn a_published_candidate_takes_the_seeds_place_with_how_it_was_made() {
    let dir = tempfile::tempdir().unwrap();
    let project = project_with_seed(dir.path(), b"old seed", &made(&[], None));

    let second = candidate_dir(&project, "second seed");
    let how = made(&["uploads"], None);
    publish(&project, &second, &how, || Ok(())).unwrap();

    assert_eq!(seed_held(&project).as_deref(), Some("second seed"));
    assert!(!second.exists(), "the seed there was is gone");
    assert!(in_force(&project, &excluding(&["uploads"])));
}

#[test]
fn a_projects_first_candidate_is_published_where_there_was_no_seed() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");

    let first = candidate_dir(&project, "first seed");
    let mut publishing = false;
    publish(&project, &first, &made(&[], None), || {
        publishing = true;
        Ok(())
    })
    .unwrap();

    assert_eq!(seed_held(&project).as_deref(), Some("first seed"));
    assert!(publishing);
}

#[test]
fn a_candidate_that_cannot_take_the_seeds_place_fails_and_leaves_it() {
    let dir = tempfile::tempdir().unwrap();
    let project = project_with_seed(dir.path(), b"seed", &made(&[], None));
    let missing = project.join(format!("{CANDIDATE_PREFIX}missing"));

    let mut publishing = false;
    let published = publish(&project, &missing, &made(&[], None), || {
        publishing = true;
        Ok(())
    });

    assert!(published.is_err());
    assert_eq!(seed_held(&project).as_deref(), Some("seed"));
    assert!(publishing, "what runs before publishing ran first");
}

#[test]
fn a_request_is_withdrawn_only_by_the_answer_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path();
    assert!(!is_wanted(store, AGENT_ID));

    let taken_under = new_request(store);
    let newer = new_request(store);
    assert!(newer > taken_under);
    withdraw(store, AGENT_ID, taken_under);
    assert!(is_wanted(store, AGENT_ID), "a newer request stands");

    withdraw(store, AGENT_ID, newer);
    assert!(!is_wanted(store, AGENT_ID));
}

#[test]
fn a_request_is_found_by_its_project_and_agent_and_forgotten_with_the_workspace() {
    let stores = tempfile::tempdir().unwrap();
    let store = stores.path().join(PROJECT_ID);
    want(&store, AGENT_ID).unwrap();
    want(&store, "chiapas").unwrap();
    let mut standing = wanted(stores.path());
    standing.sort();
    assert_eq!(
        standing,
        [
            (PROJECT_ID.to_string(), "chiapas".to_string()),
            (PROJECT_ID.to_string(), AGENT_ID.to_string()),
        ]
    );

    forget(stores.path(), AGENT_ID);
    assert!(!is_wanted(&store, AGENT_ID));
    assert!(is_wanted(&store, "chiapas"));
}

#[test]
fn a_workspace_is_needed_for_a_request_newer_than_the_one_the_seed_answers() {
    let dir = tempfile::tempdir().unwrap();
    let wanted = dir.path().join("another-store");
    let (older, answered, newer) = (
        new_request(&wanted),
        new_request(&wanted),
        new_request(&wanted),
    );
    let project = project_with_seed(dir.path(), b"seed", &made(&[], Some(answered)));
    let on = SeedSettings::default();
    let needed = |request: Option<Request>| candidate(&project, true, request).is_needed(&on);

    assert!(needed(Some(newer)));
    assert!(
        !needed(Some(older)),
        "an older merge never replaces a newer"
    );
    assert!(!needed(None), "a project with a seed needs no first one");
}

#[test]
fn a_project_without_a_seed_needs_a_workspace_that_was_asked_for_or_may_be_its_first() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let request = Some(new_request(&dir.path().join("another-store")));
    let on = SeedSettings::default();

    assert!(candidate(&project, true, None).is_needed(&on));
    assert!(!candidate(&project, true, None).is_needed(&seeds_off()));
    assert!(!candidate(&project, true, request).is_needed(&seeds_off()));

    assert!(!candidate(&project, false, None).is_needed(&on));
    assert!(candidate(&project, false, request).is_needed(&on));
}

#[test]
fn a_seed_the_engine_cannot_use_is_removed_and_the_project_needs_a_first_one() {
    let dir = tempfile::tempdir().unwrap();
    let project = project_with_seed(dir.path(), b"seed", &made(&[], None));
    let on = SeedSettings::default();
    assert!(!candidate(&project, true, None).is_needed(&on));

    let mut first = candidate(&project, true, None);
    first.at_rest.usable = false;
    assert!(first.is_needed(&on));
    assert!(!project.join(SEED).exists());
}

#[tokio::test]
async fn a_candidate_the_engine_cannot_use_is_not_made_the_seed_and_answers_its_request() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let taken = candidate_dir(&project, "taken");
    crate::project_file::set_seed_settings(PROJECT_ID, SeedSettings::default());
    let mut asked_for = candidate(&project, true, Some(new_request(&project)));
    asked_for.at_rest.usable = false;

    asked_for.settle().await;

    assert!(!is_wanted(&project, AGENT_ID));
    assert!(!project.join(SEED).exists());
    assert!(!taken.exists());
}
