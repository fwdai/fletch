use crate::capture::extract::schedule::{due, watermark, DEBOUNCE_MS};
use crate::context::model::*;

use super::{store, PROJECT};

#[test]
fn the_first_run_for_a_workspace_is_due_at_once() {
    assert!(due(None, 1_000_000, false));
}

#[test]
fn a_turn_end_inside_the_debounce_waits_and_one_past_it_runs() {
    let last = 1_000_000;
    assert!(!due(Some(last), last + DEBOUNCE_MS - 1, false));
    assert!(due(Some(last), last + DEBOUNCE_MS, false));
}

#[test]
fn an_archive_ignores_the_debounce() {
    let last = 1_000_000;
    assert!(due(Some(last), last + 1, true));
}

fn observation(
    id: &str,
    kind: SourceKind,
    reference: &str,
    workspace: &str,
    at: i64,
) -> Observation {
    Observation {
        id: id.into(),
        project_id: PROJECT.into(),
        source: Source::new(kind, Some(reference.into())),
        provenance: Provenance {
            workspace_id: Some(workspace.into()),
            ..Default::default()
        },
        input_hash: "h".into(),
        plan: None,
        created_at: at,
        extracted_at: None,
    }
}

#[test]
fn the_watermark_is_the_workspaces_latest_user_turn_observation() {
    let (store, _dir) = store();
    assert_eq!(watermark(&store, PROJECT, "ws-1").unwrap(), None);

    store
        .add_observation(&observation("o1", SourceKind::UserTurn, "t1", "ws-1", 100))
        .unwrap();
    store.mark_extracted("o1").unwrap();
    store
        .add_observation(&observation("o2", SourceKind::UserTurn, "t5", "ws-1", 300))
        .unwrap();
    store.mark_extracted("o2").unwrap();
    // Another workspace's, and a PR observation of this one: neither counts.
    store
        .add_observation(&observation("o3", SourceKind::UserTurn, "t9", "ws-2", 900))
        .unwrap();
    store
        .add_observation(&observation("o4", SourceKind::Pr, "#12", "ws-1", 950))
        .unwrap();

    let found = watermark(&store, PROJECT, "ws-1").unwrap().unwrap();
    assert_eq!(found.last_run_at, 300);
    assert_eq!(found.turn_id.as_deref(), Some("t5"));
}

/// A run that timed out or answered garbage is recorded (its observation has
/// no `extracted_at`), so it debounces the next turn-end, but the turns it
/// was given are not behind the watermark: the next run sees them again.
#[test]
fn a_failed_run_debounces_but_does_not_advance_the_turn_watermark() {
    let (store, _dir) = store();
    store
        .add_observation(&observation("o1", SourceKind::UserTurn, "t1", "ws-1", 100))
        .unwrap();
    store.mark_extracted("o1").unwrap();
    store
        .add_observation(&observation("o2", SourceKind::UserTurn, "t5", "ws-1", 300))
        .unwrap();

    let found = watermark(&store, PROJECT, "ws-1").unwrap().unwrap();
    assert_eq!(found.last_run_at, 300);
    assert_eq!(found.turn_id.as_deref(), Some("t1"));
    assert!(!due(Some(found.last_run_at), 300 + DEBOUNCE_MS - 1, false));
}

#[test]
fn only_failed_runs_so_far_means_every_turn_is_still_new() {
    let (store, _dir) = store();
    store
        .add_observation(&observation("o1", SourceKind::UserTurn, "t1", "ws-1", 100))
        .unwrap();

    let found = watermark(&store, PROJECT, "ws-1").unwrap().unwrap();
    assert_eq!(found.last_run_at, 100);
    assert_eq!(found.turn_id, None);
}
