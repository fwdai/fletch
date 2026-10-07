//! The archive trigger: it waits for a run in flight instead of dropping the
//! workspace's last turns. It settles nothing — that is the ingester's.

use std::time::Duration;

use crate::capture::extract::{claim, release, wait_for_claim, CLAIM_WAIT};

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
