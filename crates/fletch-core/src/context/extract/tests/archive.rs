//! The archive trigger: it waits for a run in flight instead of dropping the
//! workspace's last turns, and settles what is still provisional once it is
//! done.

use std::time::Duration;

use crate::context::extract::{claim, release, settle_after_archive, wait_for_claim, CLAIM_WAIT};
use crate::context::model::*;
use crate::context::{context_project_id, ContextStore};
use crate::workspace::WorkspaceManager;

use super::{agent_stamp, seed_decision_in, seed_entity_in};

#[tokio::test(start_paused = true)]
async fn an_archive_waits_for_the_run_in_flight() {
    assert!(claim("ws-wait"));
    let waiter = tokio::spawn(wait_for_claim("ws-wait", CLAIM_WAIT));
    tokio::time::sleep(Duration::from_secs(3)).await;
    release("ws-wait");
    assert!(waiter.await.unwrap());
    release("ws-wait");
}

#[tokio::test(start_paused = true)]
async fn an_archive_gives_up_on_a_run_that_never_ends() {
    assert!(claim("ws-stuck"));
    assert!(!wait_for_claim("ws-stuck", Duration::from_secs(2)).await);
    release("ws-stuck");
}

/// A workspace `id` on a real ctx, with its context project id and store.
fn workspace(ctx: &crate::host::EngineCtx, id: &str) -> (String, ContextStore) {
    let wm = WorkspaceManager::new(ctx.db.clone());
    crate::workspace::tests::agent(&wm, id, "/r", None);
    let record = wm.agent(id).unwrap();
    let project_id = context_project_id(&ctx.db.lock(), &record.project_id).unwrap();
    (project_id, ctx.context().unwrap().clone())
}

/// A provisional decision landed from `workspace_id`, as a turn-end run that
/// finished after `ingest::on_workspace_archived` leaves one.
fn provisional(store: &ContextStore, project_id: &str, workspace_id: &str, statement: &str) -> Id {
    let entity = seed_entity_in(store, project_id, &format!("e-{statement}"));
    let mut stamp = agent_stamp();
    stamp.provenance.workspace_id = Some(workspace_id.into());
    seed_decision_in(
        store,
        project_id,
        &entity,
        statement,
        stamp,
        AssertionStatus::Provisional,
    )
}

#[test]
fn an_archive_settles_what_the_workspace_still_has_provisional() {
    for (merged, expected) in [
        (true, AssertionStatus::Confirmed),
        (false, AssertionStatus::Abandoned),
    ] {
        let (ctx, _sink, _dir) = crate::host::ctx::test_ctx();
        crate::database::set_setting(&ctx.db.lock(), crate::context::DEV_SETTING, "true").unwrap();
        let (project_id, store) = workspace(&ctx, "ws-1");
        let (_, other_store) = workspace(&ctx, "ws-2");
        let ours = provisional(&store, &project_id, "ws-1", "ours");
        let theirs = provisional(&other_store, &project_id, "ws-2", "theirs");

        assert_eq!(settle_after_archive(&ctx, "ws-1", merged).unwrap(), 1);
        let graph = store.load(&project_id).unwrap();
        assert_eq!(
            graph.assertion(&ours).unwrap().status,
            expected,
            "merged={merged}"
        );
        // Another workspace's provisionals are not this archive's to settle.
        assert_eq!(
            graph.assertion(&theirs).unwrap().status,
            AssertionStatus::Provisional
        );
        // Nothing left: a second settle is a no-op.
        assert_eq!(settle_after_archive(&ctx, "ws-1", merged).unwrap(), 0);
    }
}
