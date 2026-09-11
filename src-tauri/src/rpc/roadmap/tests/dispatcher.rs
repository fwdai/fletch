use super::*;
use crate::rpc::roadmap::test_support::*;

use serde_json::{json, Value};

#[tokio::test]
async fn unknown_roadmap_ops_are_named_and_everything_else_delegates() {
    let db = test_db("p1");
    let d = dispatcher(&db, "p1");

    let resp = d.dispatch("r1", "roadmap_delete", &Value::Null).await.0;
    assert!(!resp.ok);
    let e = resp.error.unwrap();
    assert!(
        e.contains("roadmap_list") && e.contains("roadmap_propose"),
        "{e}"
    );

    // A non-roadmap op falls through to the git dispatcher untouched.
    let resp = d.dispatch("r2", "ping", &Value::Null).await.0;
    assert!(resp.ok);
    assert_eq!(resp.stdout.unwrap(), "pong");

    // And the advisory grant still refuses to publish through it.
    let resp = d.dispatch("r3", "open_pr", &json!({})).await.0;
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("roadmap item"));
}

#[test]
fn the_instruction_block_documents_exactly_these_ops() {
    // The PM only knows an op exists because the injected block says so; a
    // rename on either side is a tool the agent can't call.
    //
    // The count is pinned too, and the block's own prose states it ("nine
    // extra RPC ops"): an op added without a word about it in the block is an
    // op the agent will never call, and a number in the prose that no longer
    // matches is the first thing a reader would trust and shouldn't.
    assert_eq!(OPS.len(), 9);
    // Read without a product brief: the scan below must see the playbook's own
    // prose, not a document written by a PM (which may mention anything).
    let block = crate::instructions::roadmap_block(None).expect("shipped default is non-empty");
    assert!(
        block.contains("nine extra RPC ops"),
        "the block's own count must match OPS"
    );
    for op in OPS {
        assert!(block.contains(op), "instruction block never mentions {op}");
    }
    // And it must not promise anything this dispatcher would reject.
    for line in block.lines() {
        for word in line.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
            if word.starts_with("roadmap_") {
                assert!(
                    OPS.contains(&word),
                    "block names an op we don't implement: {word}"
                );
            }
        }
    }
}
