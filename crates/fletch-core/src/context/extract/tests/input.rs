use serde_json::{json, Value};

use crate::context::extract::input::{
    assistant_text, turns_since, MAX_HEADS_CHARS, MAX_TURNS_CHARS,
};
use crate::context::extract::{ExtractInput, TurnText};
use crate::context::fixtures;
use crate::workspace::{SessionRecord, UserTurn};

fn turn(id: &str, text: &str, native_id: Option<&str>, inherited: bool) -> UserTurn {
    UserTurn {
        turn_id: id.into(),
        seq: 0,
        text: text.into(),
        attachments: Vec::new(),
        native_id: native_id.map(str::to_string),
        started_at: None,
        ended_at: None,
        inherited,
    }
}

fn record(seq: i64, native_id: &str, body: Value) -> SessionRecord {
    SessionRecord {
        seq,
        provider: "claude".into(),
        source: "transcript".into(),
        native_id: native_id.into(),
        agent_version: None,
        body,
        inherited: false,
    }
}

fn claude_assistant(text: &str) -> Value {
    json!({ "type": "assistant", "message": { "role": "assistant", "content": [{ "type": "text", "text": text }] } })
}

fn claude_user(text: &str) -> Value {
    json!({ "type": "user", "message": { "role": "user", "content": text } })
}

#[test]
fn each_turn_gets_the_agents_final_message_before_the_next_prompt() {
    let turns = vec![
        turn("t1", "first", Some("u1"), false),
        turn("t2", "second", Some("u2"), false),
    ];
    let records = vec![
        record(1, "u1", claude_user("first")),
        record(2, "a1", claude_assistant("thinking")),
        record(3, "a2", claude_assistant("first done")),
        record(4, "u2", claude_user("second")),
        record(5, "a3", claude_assistant("second done")),
    ];
    let got = turns_since(&turns, &records, None);
    assert_eq!(
        got,
        vec![
            TurnText {
                turn_id: "t1".into(),
                user: "first".into(),
                assistant: Some("first done".into())
            },
            TurnText {
                turn_id: "t2".into(),
                user: "second".into(),
                assistant: Some("second done".into())
            },
        ]
    );
}

#[test]
fn only_turns_after_the_watermark_are_new_and_an_unknown_watermark_takes_all() {
    let turns = vec![
        turn("t0", "inherited", Some("u0"), true),
        turn("t1", "first", Some("u1"), false),
        turn("t2", "second", None, false),
    ];
    let after_t1 = turns_since(&turns, &[], Some("t1"));
    assert_eq!(after_t1.len(), 1);
    assert_eq!(after_t1[0].turn_id, "t2");
    assert_eq!(after_t1[0].assistant, None);

    let all = turns_since(&turns, &[], Some("gone"));
    assert_eq!(
        all.iter().map(|t| t.turn_id.as_str()).collect::<Vec<_>>(),
        ["t1", "t2"]
    );
    assert!(turns_since(&turns, &[], Some("t2")).is_empty());
}

#[test]
fn assistant_text_reads_the_providers_shapes_and_skips_sidechains() {
    assert_eq!(
        assistant_text(&claude_assistant(" hi ")).as_deref(),
        Some("hi")
    );
    assert_eq!(assistant_text(&claude_user("hi")), None);
    let cursor = json!({ "role": "assistant", "message": { "content": [
        { "type": "text", "text": "a" }, { "type": "tool_use" }, { "type": "text", "text": "b" }] } });
    assert_eq!(assistant_text(&cursor).as_deref(), Some("a\nb"));
    let pi = json!({ "message": { "role": "assistant", "content": [{ "type": "text", "text": "pong" }] } });
    assert_eq!(assistant_text(&pi).as_deref(), Some("pong"));
    let codex = json!({ "type": "event_msg", "payload": { "type": "item_completed",
        "item": { "type": "agent_message", "text": "done" } } });
    assert_eq!(assistant_text(&codex).as_deref(), Some("done"));
    let sidechain =
        json!({ "type": "assistant", "isSidechain": true, "message": { "content": "x" } });
    assert_eq!(assistant_text(&sidechain), None);
    let empty = json!({ "type": "assistant", "message": { "content": [] } });
    assert_eq!(assistant_text(&empty), None);
}

#[test]
fn the_oldest_turns_go_first_when_over_budget() {
    let big = "x".repeat(MAX_TURNS_CHARS / 2);
    let turns = (0..5)
        .map(|i| TurnText {
            turn_id: format!("t{i}"),
            user: big.clone(),
            assistant: None,
        })
        .collect();
    let input = ExtractInput::new(None, turns, &fixtures::graph(vec![], vec![], vec![]));
    assert_eq!(input.turns.len(), 2);
    assert_eq!(input.turns[0].turn_id, "t3");
    assert_eq!(input.last_turn_id(), Some("t4"));
    assert_eq!(input.user_chars(), MAX_TURNS_CHARS);
}

#[test]
fn heads_are_all_shown_when_small_and_only_mentioned_ones_when_not() {
    let graph = fixtures::graph(
        vec![
            fixtures::feature("f1", "auth"),
            fixtures::feature("f2", "billing"),
        ],
        vec![
            fixtures::assertion("a1", &["f1"], "Use JWT"),
            fixtures::assertion("a2", &["f2"], "Charge monthly"),
        ],
        vec![],
    );
    let turn = |text: &str| {
        vec![TurnText {
            turn_id: "t1".into(),
            user: text.into(),
            assistant: None,
        }]
    };
    let small = ExtractInput::new(None, turn("about auth"), &graph);
    assert!(
        small
            .heads
            .contains("auth: [decision/architectural/adopted] Use JWT (a1)"),
        "{}",
        small.heads
    );
    assert!(small.heads.contains("billing: "));
    assert!(small.index.contains("auth"));

    let mut big = graph.clone();
    let mut a3 = fixtures::assertion("a3", &["f2"], &"z".repeat(MAX_HEADS_CHARS));
    a3.statement.push_str(" long");
    big.assertions.push(a3);
    let filtered = ExtractInput::new(None, turn("Let's revisit Auth"), &big);
    assert!(
        filtered.heads.contains("Use JWT (a1)"),
        "{}",
        filtered.heads
    );
    assert!(!filtered.heads.contains("Charge monthly"));
}
