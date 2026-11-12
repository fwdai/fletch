use std::io;
use std::path::{Path, PathBuf};

use super::candidate::Candidate;
use super::request::{standing_request, withdraw, Request};
use super::store::{publish, seed_in_force, Made, CANDIDATE_PREFIX, MADE, SEED};
use super::{is_wanted, want, AtRest};
use crate::error::Result;
use crate::project_file::SeedSettings;

/// A workspace at rest that takes and keeps nothing: enough to ask whether
/// its project needs it.
struct Workspace {
    store: PathBuf,
    request_file: PathBuf,
    may_be_first: bool,
}

impl AtRest for Workspace {
    fn agent_id(&self) -> &str {
        "kansai"
    }

    fn project_id(&self) -> &str {
        "project"
    }

    fn store(&self) -> &Path {
        &self.store
    }

    fn request_file(&self) -> &Path {
        &self.request_file
    }

    fn may_be_first(&self) -> bool {
        self.may_be_first
    }

    fn take(&self, _candidate: &Path) -> io::Result<()> {
        Ok(())
    }

    async fn reduce(&self, _candidate: &Path, _excluded: &[String]) -> Result<u64> {
        Ok(0)
    }

    fn kept_nothing(&self) {}

    fn publishing(&self) -> io::Result<()> {
        Ok(())
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
            request_file: project.join("seed-wanted"),
            may_be_first,
        },
        dir: project.join(format!("{CANDIDATE_PREFIX}test")),
        request,
    }
}

fn new_request(request_file: &Path) -> Request {
    want(request_file).unwrap();
    standing_request(request_file).unwrap()
}

fn seed_held(project: &Path) -> Option<String> {
    std::fs::read_to_string(project.join(SEED).join("held")).ok()
}

fn in_force(project: &Path, settings: &SeedSettings) -> bool {
    seed_in_force(project, settings).is_some()
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
    let wanted = dir.path().join("seed-wanted");
    assert!(!is_wanted(&wanted));

    let taken_under = new_request(&wanted);
    let newer = new_request(&wanted);
    assert!(newer > taken_under);
    withdraw(&wanted, taken_under);
    assert!(is_wanted(&wanted), "a newer request stands");

    withdraw(&wanted, newer);
    assert!(!is_wanted(&wanted));
}

#[test]
fn a_workspace_is_needed_for_a_request_newer_than_the_one_the_seed_answers() {
    let dir = tempfile::tempdir().unwrap();
    let wanted = dir.path().join("seed-wanted");
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
    let request = Some(new_request(&dir.path().join("seed-wanted")));
    let on = SeedSettings::default();

    assert!(candidate(&project, true, None).is_needed(&on));
    assert!(!candidate(&project, true, None).is_needed(&seeds_off()));
    assert!(!candidate(&project, true, request).is_needed(&seeds_off()));

    assert!(!candidate(&project, false, None).is_needed(&on));
    assert!(candidate(&project, false, request).is_needed(&on));
}
