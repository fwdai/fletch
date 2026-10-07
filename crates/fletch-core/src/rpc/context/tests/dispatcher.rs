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

/// The project was let through at spawn; every op asks again. With the
/// layer turned off mid-session, reads and writes alike are refused by name
/// while everything else still delegates.
#[tokio::test]
async fn every_op_is_refused_once_the_layer_is_turned_off() {
    let (d, _dir) = dispatcher();
    let e = entity(&d, "billing").await;
    assert!(call(&d, "context_get", json!({})).await.ok);

    crate::database::set_setting(&db(&d).lock(), context::DEV_SETTING, "false").unwrap();
    for (op, args) in [
        ("context_get", json!({})),
        (
            "context_record_entity",
            json!({ "slug": "late", "kind": "feature", "name": "Late", "summary": "" }),
        ),
        ("context_record_decision", decision("billing", "too late")),
    ] {
        let err = error(&call(&d, op, args).await);
        assert!(err.contains("context layer is off"), "{op}: {err}");
    }
    assert_eq!(graph(&d).entities.len(), 1, "nothing landed");
    assert!(graph(&d).entity(&e).is_some());
    let resp = call(&d, "ping", Value::Null).await;
    assert_eq!(resp.stdout.as_deref(), Some("pong"));
}
