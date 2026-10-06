//! Ingest behaviour through the service over `ContextStore::temp()`. Seeds
//! and the ingester alike write through `ContextService`; the store's writes
//! are not reachable from here.

use super::*;
use crate::context::ContextStore;

const PROJECT: &str = "proj-ctx";
const WORKSPACE: &str = "ws-denali";
/// The workspace's checkouts: `A` is its primary repo.
const REPO_A: &str = "frontend";
const REPO_B: &str = "gateway";

fn service() -> (ContextService, tempfile::TempDir) {
    let (store, dir) = ContextStore::temp().unwrap();
    (ContextService::new(store.db().clone()).unwrap(), dir)
}

fn project() -> Project {
    Project {
        id: PROJECT.into(),
        fletch_id: "fp-ctx".into(),
    }
}

fn user_stamp() -> Stamp {
    Stamp {
        author: Author::user(),
        source: Source::ui(),
        provenance: Provenance::default(),
    }
}

fn agent_stamp(workspace_id: &str, repo: Option<&str>) -> Stamp {
    Stamp {
        author: Author::agent("denali", "claude"),
        source: Source::new(SourceKind::AgentTurn, Some("turn-1".into())),
        provenance: Provenance {
            workspace_id: Some(workspace_id.to_string()),
            repo: repo.map(str::to_string),
            ..Default::default()
        },
    }
}

fn seed_entity(service: &ContextService, slug: &str) -> Id {
    service
        .record_entity(
            &project(),
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

/// Seeded through the service: what is there before the merge. Named `New`
/// so an agent stamp lands next to the heads instead of being handed them.
fn seed_assertion(service: &ContextService, input: AssertionInput, stamp: Stamp) -> Id {
    let landing = service
        .record_decision(
            &project(),
            Candidate {
                input,
                user: None,
                relation: Some(ProposedRelation {
                    kind: RelationKind::New,
                    target: None,
                    reasoning: None,
                }),
                evidence: Vec::new(),
                about_pending: Vec::new(),
                observation_id: None,
            },
            stamp,
        )
        .unwrap();
    match landing {
        Landing::Recorded { id, .. } => id,
        other => panic!("seed did not land: {other:?}"),
    }
}

/// A provisional fact about `about`, recorded from `workspace_id` in `repo`.
fn provisional(service: &ContextService, about: &Id, workspace_id: &str, repo: Option<&str>) -> Id {
    seed_assertion(
        service,
        assertion(
            std::slice::from_ref(about),
            AssertionKind::Fact,
            &format!("From {workspace_id} in {repo:?}."),
            AssertionStatus::Provisional,
        ),
        agent_stamp(workspace_id, repo),
    )
}

fn merged(body: &str) -> MergedPr {
    MergedPr {
        reference: "https://github.com/o/r/pull/7".to_string(),
        body: body.to_string(),
        branch: Some("feat/sessions".to_string()),
        sha: Some("abc123".to_string()),
    }
}

/// [`ingest_merged`] for `WORKSPACE`'s primary checkout.
fn merge_primary(service: &ContextService, pr: &MergedPr) -> Result<()> {
    ingest_merged(service, &project(), WORKSPACE, REPO_A, true, pr)
}

fn graph(service: &ContextService) -> Graph {
    service.store().load(PROJECT).unwrap()
}

fn status(service: &ContextService, id: &str) -> AssertionStatus {
    graph(service).assertion(id).unwrap().status
}

fn current(service: &ContextService) -> Vec<Assertion> {
    let graph = graph(service);
    graph
        .assertions
        .iter()
        .filter(|a| graph.current.contains(&a.id))
        .cloned()
        .collect()
}

fn pending(service: &ContextService) -> Vec<Proposal> {
    service
        .store()
        .proposals(PROJECT, Some(ProposalStatus::Pending))
        .unwrap()
}

#[test]
fn a_merge_records_the_prs_decisions_through_the_service() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth-session");
    seed_entity(&service, "login");

    merge_primary(
        &service,
        &merged(
            "## Summary\nx\n## Decisions\n\
             - adopted · implementation · [auth-session, login] — Sessions refresh server-side. — Client refresh raced.\n\
             - constraint · business · [auth-session] — Sessions expire after a day.\n",
        ),
    )
    .unwrap();

    let graph = graph(&service);
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
    assert_eq!(a.provenance.repo.as_deref(), Some(REPO_A));
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
    let (service, _dir) = service();
    seed_entity(&service, "auth-session");

    merge_primary(
        &service,
        &merged(
            "## Decisions\n\
             - adopted · implementation · [auth-session, nowhere] — Skipped: one slug is unknown.\n\
             - adopted · implementation — Skipped: no entities.\n\
             - adopted · implementation · [auth-session] — Kept.\n",
        ),
    )
    .unwrap();

    let statements: Vec<_> = current(&service).into_iter().map(|a| a.statement).collect();
    assert_eq!(statements, ["Kept."]);
    assert!(
        graph(&service).entities.len() == 1,
        "no entity is created from a PR"
    );
}

#[test]
fn a_duplicate_of_a_head_is_skipped() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth-session");
    seed_assertion(
        &service,
        assertion(
            &[auth],
            AssertionKind::Decision,
            "Sessions refresh server-side.",
            AssertionStatus::Confirmed,
        ),
        user_stamp(),
    );

    merge_primary(
        &service,
        &merged("## Decisions\n- adopted · implementation · [auth-session] — sessions refresh   server-side\n"),
    )
    .unwrap();

    assert_eq!(graph(&service).assertions.len(), 1);
    assert!(pending(&service).is_empty());
}

/// A line that says something different next to a user-stated head is the
/// policy's to land or hold (`resolve::auto_rule`): either way it is not
/// lost, and the head stays.
#[test]
fn a_different_statement_lands_or_is_held_by_the_rule() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth-session");
    let old = seed_assertion(
        &service,
        assertion(
            &[auth],
            AssertionKind::Decision,
            "Refresh in the client.",
            AssertionStatus::Confirmed,
        ),
        user_stamp(),
    );

    merge_primary(
        &service,
        &merged("## Decisions\n- adopted · implementation · [auth-session] — Refresh server-side. — The client raced the cache.\n"),
    )
    .unwrap();

    let graph = graph(&service);
    assert!(graph.current.contains(&old));
    let landed = graph
        .assertions
        .iter()
        .find(|a| a.statement == "Refresh server-side.");
    let held = pending(&service).into_iter().find(|p| {
        matches!(&p.payload, ProposalPayload::Assertion { input, .. } if input.statement == "Refresh server-side.")
    });
    match (landed, held) {
        (Some(new), None) => {
            assert_eq!(new.status, AssertionStatus::Confirmed);
            assert_eq!(new.supersedes, None);
            assert!(new.contradicts.is_empty());
        }
        (None, Some(proposal)) => {
            assert_eq!(proposal.evidence.len(), 1, "the line is the evidence");
        }
        (landed, held) => {
            panic!("expected exactly one of landed / held, got {landed:?} / {held:?}")
        }
    }
}

/// A merge settles the checkout it merged, not the workspace: the primary
/// repo's merge takes its own records and the unstamped ones with it,
/// another repo's stay provisional until that repo's PR merges.
#[test]
fn a_merge_settles_only_the_merged_checkouts_provisionals() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth-session");
    let in_a = provisional(&service, &auth, WORKSPACE, Some(REPO_A));
    let in_b = provisional(&service, &auth, WORKSPACE, Some(REPO_B));
    let unstamped = provisional(&service, &auth, WORKSPACE, None);
    let theirs = provisional(&service, &auth, "ws-other", Some(REPO_A));

    merge_primary(&service, &merged("## Decisions\n- none\n")).unwrap();

    assert_eq!(status(&service, &in_a), AssertionStatus::Confirmed);
    assert_eq!(status(&service, &unstamped), AssertionStatus::Confirmed);
    assert_eq!(status(&service, &in_b), AssertionStatus::Provisional);
    assert_eq!(status(&service, &theirs), AssertionStatus::Provisional);

    ingest_merged(
        &service,
        &project(),
        WORKSPACE,
        REPO_B,
        false,
        &MergedPr {
            reference: pr_reference(None, REPO_B, 8),
            ..merged("## Decisions\n- none\n")
        },
    )
    .unwrap();
    assert_eq!(status(&service, &in_b), AssertionStatus::Confirmed);
    assert_eq!(status(&service, &theirs), AssertionStatus::Provisional);
}

/// A non-primary repo's merge does not take the unstamped records: those
/// are the primary checkout's.
#[test]
fn a_secondary_merge_leaves_unstamped_records_to_the_primary() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth-session");
    let in_b = provisional(&service, &auth, WORKSPACE, Some(REPO_B));
    let unstamped = provisional(&service, &auth, WORKSPACE, None);

    ingest_merged(
        &service,
        &project(),
        WORKSPACE,
        REPO_B,
        false,
        &merged("## Decisions\n- none\n"),
    )
    .unwrap();

    assert_eq!(status(&service, &in_b), AssertionStatus::Confirmed);
    assert_eq!(status(&service, &unstamped), AssertionStatus::Provisional);
}

#[test]
fn a_second_merge_with_the_same_reference_is_a_no_op() {
    let (service, _dir) = service();
    seed_entity(&service, "auth-session");
    let pr = merged("## Decisions\n- adopted · implementation · [auth-session] — Once.\n");

    merge_primary(&service, &pr).unwrap();
    let after_first = service.store().events(PROJECT).unwrap().len();
    merge_primary(&service, &pr).unwrap();
    // Same reference, different body: still the same PR, still not re-read.
    let mut edited = pr.clone();
    edited.body = "## Decisions\n- adopted · implementation · [auth-session] — Twice.\n".into();
    merge_primary(&service, &edited).unwrap();

    assert_eq!(service.store().events(PROJECT).unwrap().len(), after_first);
    assert_eq!(current(&service).len(), 1);
}

/// At archive every checkout is settled by its own fate: the merged one
/// confirms, the unmerged one abandons, and only this workspace's records.
#[test]
fn an_archive_settles_each_checkout_by_its_own_fate() {
    let (service, _dir) = service();
    let auth = seed_entity(&service, "auth-session");
    let in_a = provisional(&service, &auth, WORKSPACE, Some(REPO_A));
    let in_b = provisional(&service, &auth, WORKSPACE, Some(REPO_B));
    let unstamped = provisional(&service, &auth, WORKSPACE, None);
    let theirs = provisional(&service, &auth, "ws-open", Some(REPO_A));

    // What `on_workspace_archived` does per repo, with A merged and B not.
    let archive_stamp = |repo: &str| stamp(WORKSPACE, repo, None, None, None);
    assert_eq!(
        service
            .settle(
                &project(),
                WORKSPACE,
                REPO_A,
                true,
                true,
                archive_stamp(REPO_A)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        service
            .settle(
                &project(),
                WORKSPACE,
                REPO_B,
                false,
                false,
                archive_stamp(REPO_B)
            )
            .unwrap(),
        1
    );

    assert_eq!(status(&service, &in_a), AssertionStatus::Confirmed);
    assert_eq!(status(&service, &unstamped), AssertionStatus::Confirmed);
    assert_eq!(status(&service, &in_b), AssertionStatus::Abandoned);
    assert_eq!(status(&service, &theirs), AssertionStatus::Provisional);
    // Settling again finds nothing provisional.
    assert_eq!(
        service
            .settle(
                &project(),
                WORKSPACE,
                REPO_A,
                true,
                true,
                archive_stamp(REPO_A)
            )
            .unwrap(),
        0
    );
}

#[test]
fn a_failure_part_way_leaves_the_pr_unseen_and_a_retry_lands_the_rest() {
    let (service, _dir) = service();
    seed_entity(&service, "auth-session");
    let pr = merged(
        "## Decisions\n\
         - adopted · implementation · [auth-session] — First.\n\
         - adopted · implementation · [auth-session] — Second.\n",
    );
    // The second line's event fails to write, as a dropped connection or a
    // full disk would make it.
    service.with_conn(|conn| {
        conn.execute_batch(
            "CREATE TRIGGER context.fail_second BEFORE INSERT ON events
               WHEN NEW.payload LIKE '%Second.%'
             BEGIN SELECT RAISE(ABORT, 'simulated write failure'); END;",
        )
        .unwrap()
    });

    assert!(merge_primary(&service, &pr).is_err());
    let statements: Vec<_> = current(&service).into_iter().map(|a| a.statement).collect();
    assert_eq!(statements, ["First."]);
    assert!(
        !seen(&service, PROJECT, &pr.reference).unwrap(),
        "a PR whose lines did not all land is not marked as ingested"
    );

    service.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER context.fail_second")
            .unwrap()
    });
    merge_primary(&service, &pr).unwrap();

    let mut statements: Vec<_> = current(&service).into_iter().map(|a| a.statement).collect();
    statements.sort();
    assert_eq!(statements, ["First.", "Second."]);
    assert!(seen(&service, PROJECT, &pr.reference).unwrap());
    // A third announcement is the usual no-op.
    let events = service.store().events(PROJECT).unwrap().len();
    merge_primary(&service, &pr).unwrap();
    assert_eq!(service.store().events(PROJECT).unwrap().len(), events);
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
    let (service, _dir) = service();
    seed_entity(&service, "auth-session");
    let pr = |subdir: &str, statement: &str| MergedPr {
        reference: pr_reference(None, subdir, 7),
        ..merged(&format!(
            "## Decisions\n- adopted · implementation · [auth-session] — {statement}\n"
        ))
    };

    ingest_merged(
        &service,
        &project(),
        WORKSPACE,
        REPO_A,
        true,
        &pr(REPO_A, "From the frontend."),
    )
    .unwrap();
    ingest_merged(
        &service,
        &project(),
        WORKSPACE,
        REPO_B,
        false,
        &pr(REPO_B, "From the gateway."),
    )
    .unwrap();

    let mut statements: Vec<_> = current(&service).into_iter().map(|a| a.statement).collect();
    statements.sort();
    assert_eq!(statements, ["From the frontend.", "From the gateway."]);
}

#[test]
fn a_pr_seen_by_the_watcher_and_again_at_archive_is_ingested_once() {
    let (service, _dir) = service();
    seed_entity(&service, "auth-session");
    let url = "https://github.com/o/frontend/pull/7";
    let body = "## Decisions\n- adopted · implementation · [auth-session] — Once.\n";
    // The watcher has the PR's state (its URL); the archive path has the
    // repo record's snapshot of it. Both resolve to the same reference.
    let watched = MergedPr {
        reference: pr_reference(Some(url), REPO_A, 7),
        ..merged(body)
    };
    let archived = MergedPr {
        reference: pr_reference(Some(url), REPO_A, 7),
        ..merged(body)
    };
    assert_eq!(watched.reference, archived.reference);

    merge_primary(&service, &watched).unwrap();
    let events = service.store().events(PROJECT).unwrap().len();
    merge_primary(&service, &archived).unwrap();

    assert_eq!(service.store().events(PROJECT).unwrap().len(), events);
    assert_eq!(current(&service).len(), 1);
}
