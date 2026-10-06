use super::test_support::*;
use super::*;

use serde_json::{json, Value};

#[tokio::test]
async fn unknown_context_ops_are_named_and_everything_else_delegates() {
    let (d, _dir) = dispatcher();

    let e = error(&call(&d, "context_delete", Value::Null).await);
    for op in OPS {
        assert!(e.contains(op), "{e}");
    }

    let resp = call(&d, "ping", Value::Null).await;
    assert_eq!(resp.stdout.as_deref(), Some("pong"));

    let e = error(&call(&d, "open_pr", json!({})).await);
    assert_eq!(
        e, "unknown op: open_pr",
        "inner's own refusal passes through"
    );
}

#[test]
fn the_instruction_block_documents_exactly_these_ops() {
    assert_eq!(OPS.len(), 4);
    let block = crate::instructions::context_block(Some("")).expect("shipped default is non-empty");
    assert!(
        block.contains("Four extra RPC ops"),
        "the block's own count must match OPS"
    );
    for op in OPS {
        assert!(block.contains(op), "instruction block never mentions {op}");
    }
    for line in block.lines() {
        for word in line.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
            if word.starts_with("context_") {
                assert!(
                    OPS.contains(&word),
                    "block names an op we don't implement: {word}"
                );
            }
        }
    }
}
