
use super::*;
use crate::roadmap::types::{Horizon, ItemSource};

fn item(status: ItemStatus, pr_number: Option<i64>) -> RoadmapItem {
    RoadmapItem {
        id: "i1".into(),
        project_id: "p1".into(),
        code: "FLT-1".into(),
        title: "t".into(),
        why: String::new(),
        horizon: Horizon::Now,
        status,
        rank: 1.0,
        area: None,
        source: ItemSource::User,
        accept: Vec::new(),
        deps: Vec::new(),
        agent_id: None,
        workflow_def_id: None,
        run_id: None,
        pr_url: pr_number.map(|n| format!("https://github.com/o/r/pull/{n}")),
        pr_number,
        created_at: 1,
        hold_reason: None,
        held_by: None,
        held_at: None,
        close_reason: None,
        issue_url: None,
        updated_at: 1,
    }
}

#[test]
fn only_in_review_items_with_a_number_are_watchable() {
    assert_eq!(watchable(&item(ItemStatus::InReview, Some(42))), Some(42));
    assert_eq!(watchable(&item(ItemStatus::InReview, None)), None);
    for status in [
        ItemStatus::Proposed,
        ItemStatus::Open,
        ItemStatus::Queued,
        ItemStatus::Active,
        ItemStatus::Done,
    ] {
        assert_eq!(
            watchable(&item(status, Some(42))),
            None,
            "{} is not under review",
            status.as_str()
        );
    }
}

#[test]
fn a_nonsense_number_is_not_watchable() {
    assert_eq!(watchable(&item(ItemStatus::InReview, Some(-1))), None);
    assert_eq!(
        watchable(&item(ItemStatus::InReview, Some(i64::from(u32::MAX) + 1))),
        None
    );
}

#[test]
fn a_failed_read_degrades_to_no_answer() {
    let ok: crate::error::Result<Option<u8>> = Ok(Some(7));
    assert_eq!(degrade(ok, 1, "x"), Some(7));
    let empty: crate::error::Result<Option<u8>> = Ok(None);
    assert_eq!(degrade(empty, 1, "x"), None);
    let failed: crate::error::Result<Option<u8>> = Err(crate::error::Error::Gh("no token".into()));
    assert_eq!(degrade(failed, 1, "x"), None);
}

#[test]
fn the_feedback_note_counts_threads() {
    assert_eq!(feedback_detail(1), "Sent 1 review thread to an agent");
    assert_eq!(feedback_detail(3), "Sent 3 review threads to an agent");
}
