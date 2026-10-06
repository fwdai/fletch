//! Store-level ingest behaviour over `ContextStore::temp()`. These cross into
//! the store and resolve implementations and run only once those land.

use super::*;

const PROJECT: &str = "proj-ctx";
const WORKSPACE: &str = "ws-denali";

fn user_stamp() -> Stamp {
    Stamp {
        author: Author::user(),
        source: Source::ui(),
        provenance: Provenance::default(),
    }
}

fn agent_stamp(workspace_id: &str) -> Stamp {
    Stamp {
        author: Author::agent("denali", "claude"),
        source: Source::new(SourceKind::AgentTurn, Some("turn-1".into())),
        provenance: Provenance {
            workspace_id: Some(workspace_id.to_string()),
            ..Default::default()
        },
    }
}

fn seed_entity(store: &ContextStore, slug: &str) -> Id {
    store
        .record_entity(
            PROJECT,
            EntityInput {
                id: None,
                slug: slug.to_string(),
                kind: EntityKind::Feature,
                name: slug.to_string(),
                summary: String::new(),
                aliases: Vec::new(),
                paths: Vec::new(),
            },
            user_stamp(),
        )
        .unwrap()
}

fn assertion(
    about: &[Id],
    kind: AssertionKind,
    statement: &str,
    status: AssertionStatus,
) -> AssertionInput {
    AssertionInput {
        kind,
        domain: Domain::Implementation,
        stance: Stance::Adopted,
        statement: statement.to_string(),
        rationale: "seeded".to_string(),
        valid_from: None,
        paths: Vec::new(),
        about: about.to_vec(),
        supersedes: None,
        contradicts: Vec::new(),
        status,
    }
}

fn merged(body: &str) -> MergedPr {
    MergedPr {
        reference: "https://github.com/o/r/pull/7".to_string(),
        body: body.to_string(),
        branch: Some("feat/sessions".to_string()),
        sha: Some("abc123".to_string()),
    }
}

fn heads(store: &ContextStore) -> Vec<Assertion> {
    store
        .load(PROJECT)
        .unwrap()
        .assertions
        .into_iter()
        .filter(Assertion::is_head)
        .collect()
}

#[test]
fn a_merge_records_confirmed_assertions_from_the_pr() {
    let (store, _dir) = ContextStore::temp().unwrap();
    let auth = seed_entity(&store, "auth-session");
    seed_entity(&store, "login");

    ingest_merged(
        &store,
        PROJECT,
        WORKSPACE,
        &merged(
            "## Summary\nx\n## Decisions\n\
             - adopted · implementation · [auth-session, login] — Sessions refresh server-side. — Client refresh raced.\n\
             - constraint · business · [auth-session] — Sessions expire after a day.\n",
        ),
    )
    .unwrap();

    let graph = store.load(PROJECT).unwrap();
    assert_eq!(graph.assertions.len(), 2, "{:?}", graph.assertions);
    let a = graph
        .assertions
        .iter()
        .find(|a| a.statement == "Sessions refresh server-side.")
        .unwrap();
    assert_eq!(a.status, AssertionStatus::Confirmed);
    assert_eq!(a.author, Author::ingester());
    assert_eq!(
        a.source,
        Source::new(SourceKind::Pr, Some("https://github.com/o/r/pull/7".into()))
    );
    assert_eq!(a.provenance.workspace_id.as_deref(), Some(WORKSPACE));
    assert_eq!(a.provenance.branch.as_deref(), Some("feat/sessions"));
    assert_eq!(a.provenance.commit_sha.as_deref(), Some("abc123"));
    assert_eq!(a.rationale, "Client refresh raced.");
    assert!(a.about.contains(&auth));
    assert_eq!(a.about.len(), 2);
    let c = graph
        .assertions
        .iter()
        .find(|a| a.kind == AssertionKind::Constraint)
        .unwrap();
    assert_eq!(c.domain, Domain::Business);
    assert_eq!(c.about, vec![auth]);
}

#[test]
fn a_line_about_an_unknown_slug_is_skipped() {
    let (store, _dir) = ContextStore::temp().unwrap();
    seed_entity(&store, "auth-session");

    ingest_merged(
        &store,
        PROJECT,
        WORKSPACE,
        &merged(
            "## Decisions\n\
             - adopted · implementation · [auth-session, nowhere] — Skipped: one slug is unknown.\n\
             - adopted · implementation — Skipped: no entities.\n\
             - adopted · implementation · [auth-session] — Kept.\n",
        ),
    )
    .unwrap();

    let statements: Vec<_> = heads(&store).into_iter().map(|a| a.statement).collect();
    assert_eq!(statements, ["Kept."]);
    assert!(
        store.load(PROJECT).unwrap().entities.len() == 1,
        "no entity is created from a PR"
    );
}

#[test]
fn a_duplicate_of_a_head_is_skipped() {
    let (store, _dir) = ContextStore::temp().unwrap();
    let auth = seed_entity(&store, "auth-session");
    store
        .record_assertion(
            PROJECT,
            assertion(
                &[auth],
                AssertionKind::Decision,
                "Sessions refresh server-side.",
                AssertionStatus::Confirmed,
            ),
            user_stamp(),
        )
        .unwrap();

    ingest_merged(
        &store,
        PROJECT,
        WORKSPACE,
        &merged("## Decisions\n- adopted · implementation · [auth-session] — sessions refresh   server-side\n"),
    )
    .unwrap();

    assert_eq!(store.load(PROJECT).unwrap().assertions.len(), 1);
    assert!(store
        .proposals(PROJECT, Some(ProposalStatus::Pending))
        .unwrap()
        .is_empty());
}

#[test]
fn a_different_statement_lands_next_to_the_existing_head() {
    let (store, _dir) = ContextStore::temp().unwrap();
    let auth = seed_entity(&store, "auth-session");
    let old = store
        .record_assertion(
            PROJECT,
            assertion(
                &[auth],
                AssertionKind::Decision,
                "Refresh in the client.",
                AssertionStatus::Confirmed,
            ),
            user_stamp(),
        )
        .unwrap();

    ingest_merged(
        &store,
        PROJECT,
        WORKSPACE,
        &merged("## Decisions\n- adopted · implementation · [auth-session] — Refresh server-side. — The client raced the cache.\n"),
    )
    .unwrap();

    // Whether the new line replaces or contradicts the old one is the
    // user's call: both stay heads, nothing is parked for review.
    let graph = store.load(PROJECT).unwrap();
    let new = graph
        .assertions
        .iter()
        .find(|a| a.statement == "Refresh server-side.")
        .unwrap();
    assert_eq!(new.status, AssertionStatus::Confirmed);
    assert_eq!(new.supersedes, None);
    assert!(new.contradicts.is_empty());
    assert!(graph.assertion(&old).unwrap().is_head());
    assert_eq!(heads(&store).len(), 2);
    assert!(store
        .proposals(PROJECT, Some(ProposalStatus::Pending))
        .unwrap()
        .is_empty());
}

#[test]
fn the_workspaces_provisional_assertions_are_confirmed_on_merge() {
    let (store, _dir) = ContextStore::temp().unwrap();
    let auth = seed_entity(&store, "auth-session");
    let mine = store
        .record_assertion(
            PROJECT,
            assertion(
                std::slice::from_ref(&auth),
                AssertionKind::Fact,
                "Recorded mid-branch.",
                AssertionStatus::Provisional,
            ),
            agent_stamp(WORKSPACE),
        )
        .unwrap();
    let theirs = store
        .record_assertion(
            PROJECT,
            assertion(
                &[auth],
                AssertionKind::Fact,
                "Another branch's.",
                AssertionStatus::Provisional,
            ),
            agent_stamp("ws-other"),
        )
        .unwrap();

    ingest_merged(
        &store,
        PROJECT,
        WORKSPACE,
        &merged("## Decisions\n- none\n"),
    )
    .unwrap();

    let graph = store.load(PROJECT).unwrap();
    assert_eq!(
        graph.assertion(&mine).unwrap().status,
        AssertionStatus::Confirmed
    );
    assert_eq!(
        graph.assertion(&theirs).unwrap().status,
        AssertionStatus::Provisional
    );
    assert!(store
        .provisional_for_workspace(PROJECT, WORKSPACE)
        .unwrap()
        .is_empty());
}

#[test]
fn a_second_merge_with_the_same_reference_is_a_no_op() {
    let (store, _dir) = ContextStore::temp().unwrap();
    seed_entity(&store, "auth-session");
    let pr = merged("## Decisions\n- adopted · implementation · [auth-session] — Once.\n");

    ingest_merged(&store, PROJECT, WORKSPACE, &pr).unwrap();
    let after_first = store.events(PROJECT).unwrap().len();
    ingest_merged(&store, PROJECT, WORKSPACE, &pr).unwrap();
    // Same reference, different body: still the same PR, still not re-read.
    let mut edited = pr.clone();
    edited.body = "## Decisions\n- adopted · implementation · [auth-session] — Twice.\n".into();
    ingest_merged(&store, PROJECT, WORKSPACE, &edited).unwrap();

    assert_eq!(store.events(PROJECT).unwrap().len(), after_first);
    assert_eq!(heads(&store).len(), 1);
}

#[test]
fn archiving_without_a_merge_abandons_and_with_one_confirms() {
    let (store, _dir) = ContextStore::temp().unwrap();
    let auth = seed_entity(&store, "auth-session");
    let record = |ws: &str, statement: &str| {
        store
            .record_assertion(
                PROJECT,
                assertion(
                    std::slice::from_ref(&auth),
                    AssertionKind::Fact,
                    statement,
                    AssertionStatus::Provisional,
                ),
                agent_stamp(ws),
            )
            .unwrap()
    };
    let dropped = record(WORKSPACE, "Never merged.");
    let kept = record("ws-landed", "Merged while the host slept.");
    let untouched = record("ws-open", "Still open.");

    let archive_stamp = |ws: &str| stamp(ws, None, None, None);
    assert_eq!(
        settle_workspace(&store, PROJECT, WORKSPACE, false, &archive_stamp(WORKSPACE)).unwrap(),
        1
    );
    assert_eq!(
        settle_workspace(
            &store,
            PROJECT,
            "ws-landed",
            true,
            &archive_stamp("ws-landed")
        )
        .unwrap(),
        1
    );

    let graph = store.load(PROJECT).unwrap();
    assert_eq!(
        graph.assertion(&dropped).unwrap().status,
        AssertionStatus::Abandoned
    );
    assert_eq!(
        graph.assertion(&kept).unwrap().status,
        AssertionStatus::Confirmed
    );
    assert_eq!(
        graph.assertion(&untouched).unwrap().status,
        AssertionStatus::Provisional
    );
    // Settling again finds nothing provisional.
    assert_eq!(
        settle_workspace(&store, PROJECT, WORKSPACE, false, &archive_stamp(WORKSPACE)).unwrap(),
        0
    );
}

#[test]
fn a_failure_part_way_leaves_the_pr_unseen_and_a_retry_lands_the_rest() {
    let (store, _dir) = ContextStore::temp().unwrap();
    seed_entity(&store, "auth-session");
    let pr = merged(
        "## Decisions\n\
         - adopted · implementation · [auth-session] — First.\n\
         - adopted · implementation · [auth-session] — Second.\n",
    );
    // The second line's event fails to write, as a dropped connection or a
    // full disk would make it.
    store
        .db()
        .lock()
        .execute_batch(
            "CREATE TRIGGER context.fail_second BEFORE INSERT ON events
               WHEN NEW.payload LIKE '%Second.%'
             BEGIN SELECT RAISE(ABORT, 'simulated write failure'); END;",
        )
        .unwrap();

    assert!(ingest_merged(&store, PROJECT, WORKSPACE, &pr).is_err());
    let statements: Vec<_> = heads(&store).into_iter().map(|a| a.statement).collect();
    assert_eq!(statements, ["First."]);
    assert!(
        !seen(&store, PROJECT, &pr.reference).unwrap(),
        "a PR whose lines did not all land is not marked as ingested"
    );

    store
        .db()
        .lock()
        .execute_batch("DROP TRIGGER context.fail_second")
        .unwrap();
    ingest_merged(&store, PROJECT, WORKSPACE, &pr).unwrap();

    let mut statements: Vec<_> = heads(&store).into_iter().map(|a| a.statement).collect();
    statements.sort();
    assert_eq!(statements, ["First.", "Second."]);
    assert!(seen(&store, PROJECT, &pr.reference).unwrap());
    // A third announcement is the usual no-op.
    let events = store.events(PROJECT).unwrap().len();
    ingest_merged(&store, PROJECT, WORKSPACE, &pr).unwrap();
    assert_eq!(store.events(PROJECT).unwrap().len(), events);
}

#[test]
fn the_reference_is_the_url_when_known_and_the_subdir_and_number_otherwise() {
    let url = "https://github.com/o/frontend/pull/7";
    assert_eq!(pr_reference(Some(url), "frontend", 7), url);
    // The watcher reports an unknown URL as an empty string.
    assert_eq!(pr_reference(Some(""), "frontend", 7), "frontend#7");
    assert_eq!(pr_reference(None, "frontend", 7), "frontend#7");
    assert_ne!(
        pr_reference(None, "frontend", 7),
        pr_reference(None, "gateway", 7)
    );
}

#[test]
fn the_same_pr_number_in_two_repos_ingests_both() {
    let (store, _dir) = ContextStore::temp().unwrap();
    seed_entity(&store, "auth-session");
    let pr = |subdir: &str, statement: &str| MergedPr {
        reference: pr_reference(None, subdir, 7),
        ..merged(&format!(
            "## Decisions\n- adopted · implementation · [auth-session] — {statement}\n"
        ))
    };

    ingest_merged(
        &store,
        PROJECT,
        WORKSPACE,
        &pr("frontend", "From the frontend."),
    )
    .unwrap();
    ingest_merged(
        &store,
        PROJECT,
        WORKSPACE,
        &pr("gateway", "From the gateway."),
    )
    .unwrap();

    let mut statements: Vec<_> = heads(&store).into_iter().map(|a| a.statement).collect();
    statements.sort();
    assert_eq!(statements, ["From the frontend.", "From the gateway."]);
}

#[test]
fn a_pr_seen_by_the_watcher_and_again_at_archive_is_ingested_once() {
    let (store, _dir) = ContextStore::temp().unwrap();
    seed_entity(&store, "auth-session");
    let url = "https://github.com/o/frontend/pull/7";
    let body = "## Decisions\n- adopted · implementation · [auth-session] — Once.\n";
    // The watcher has the PR's state (its URL); the archive path has the
    // repo record's snapshot of it. Both resolve to the same reference.
    let watched = MergedPr {
        reference: pr_reference(Some(url), "frontend", 7),
        ..merged(body)
    };
    let archived = MergedPr {
        reference: pr_reference(Some(url), "frontend", 7),
        ..merged(body)
    };
    assert_eq!(watched.reference, archived.reference);

    ingest_merged(&store, PROJECT, WORKSPACE, &watched).unwrap();
    let events = store.events(PROJECT).unwrap().len();
    ingest_merged(&store, PROJECT, WORKSPACE, &archived).unwrap();

    assert_eq!(store.events(PROJECT).unwrap().len(), events);
    assert_eq!(heads(&store).len(), 1);
}
