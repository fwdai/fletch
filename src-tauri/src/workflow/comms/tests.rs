use super::answer::deliver_answer;
use super::route::route;
use super::sender::{resolve_caps, Poke};
use super::*;
use serde_json::json;
use crate::workflow::spec::{
    AgentSpec, Block, ChildTemplate, ComposeLimits, Gate, Integrate, Join, Orchestrate, Spec, Step,
};
use crate::workflow::types::{MessageKind, MessageStatus};
use std::collections::BTreeMap;

// ── caps matrix (spec §10.1) ──────────────────────────────────────────

#[test]
fn publish_ops_are_denied_for_run_owned_agents() {
    // §15: the dispatcher short-circuits these before the GitDispatcher
    // fallthrough — a step agent can never push or open a PR with host
    // credentials. `git_fetch` (and unknown ops) still fall through.
    assert!(is_publish_op("git_push"));
    assert!(is_publish_op("open_pr"));
    assert!(!is_publish_op("git_fetch"));
    assert!(!is_publish_op("wf_report"));
    assert!(!is_publish_op("echo"));
}

#[test]
fn cap_for_op_maps_the_three_comms_ops() {
    assert_eq!(cap_for_op("wf_report"), Some(CommsCap::Report));
    assert_eq!(cap_for_op("wf_ask"), Some(CommsCap::Ask));
    assert_eq!(cap_for_op("wf_notify"), Some(CommsCap::Notify));
    assert_eq!(cap_for_op("git_push"), None);
    assert!(!is_comms_op("git_push"));
    assert!(is_comms_op("wf_ask"));
}

#[test]
fn check_cap_matrix() {
    assert!(check_cap("wf_report", &[CommsCap::Report]).is_ok());
    assert!(check_cap("wf_report", &[CommsCap::Ask]).is_err());
    assert!(check_cap("wf_ask", &[CommsCap::Ask]).is_ok());
    assert!(check_cap("wf_ask", &[]).is_err());
    // notify is never grantable to a step, so it's always rejected here.
    assert!(check_cap("wf_notify", &[CommsCap::Report, CommsCap::Ask]).is_err());
    assert!(check_cap("wf_notify", &[CommsCap::Notify]).is_ok());
    assert!(check_cap("rm_rf", &[CommsCap::Report]).is_err());
}

// ── delivery coalescing (spec §10.4) ──────────────────────────────────

fn msg(kind: MessageKind, body: Value) -> Message {
    Message {
        id: "m".into(),
        run_id: "r".into(),
        from_step_exec_id: None,
        to_step_exec_id: Some("e".into()),
        kind,
        body,
        status: MessageStatus::Queued,
        created_at: 0,
        delivered_at: None,
    }
}

#[test]
fn compose_delivery_coalesces_many_messages_into_one_preamble() {
    let msgs = vec![
        msg(MessageKind::Answer, json!({ "text": "use Postgres" })),
        msg(MessageKind::Notify, json!({ "message": "slice B landed" })),
    ];
    let p = compose_delivery(&msgs);
    assert_eq!(p.matches("## Messages from the workflow").count(), 1);
    assert!(p.contains("use Postgres"));
    assert!(p.contains("slice B landed"));
}

#[test]
fn compose_delivery_renders_answer_body() {
    let p = compose_delivery(&[msg(MessageKind::Answer, json!({ "text": "yes, ship it" }))]);
    assert!(p.contains("Answer to your question:"));
    assert!(p.contains("yes, ship it"));
}

// ── routing over a temp DB (spec §10.1, §10.4) ────────────────────────

/// A DB with one paused run, one live step attempt, and a spec whose single
/// step declares `caps`. Returns (db, run_id, step_exec_id).
fn seed(caps: Vec<CommsCap>) -> (Connection, String, String) {
    let td = tempfile::tempdir().unwrap();
    let db = crate::database::init(td.path()).unwrap();
    // Keep the tempdir alive for the connection's lifetime by leaking it —
    // acceptable in a unit test.
    std::mem::forget(td);
    let conn = Arc::try_unwrap(db).ok().unwrap().into_inner();

    let mut agents = BTreeMap::new();
    agents.insert(
        "a".to_string(),
        AgentSpec {
            base: "claude".into(),
            model: None,
            effort: None,
            instructions: None,
            skills: vec![],
            mcp_servers: vec![],
            custom_agent: None,
        },
    );
    let spec = Spec {
        version: 1,
        name: "demo".into(),
        description: None,
        budgets: None,
        agents,
        workflow: vec![Block::Step(Step {
            id: "s1".into(),
            agent: "a".into(),
            goal: "g".into(),
            gate: Gate::Verdict,
            budgets: None,
            comms: caps,
        })],
        finalize: None,
    };
    let spec_json = serde_json::to_string(&spec).unwrap();
    conn.execute(
        "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
            base_sha,status,paused_reason,budgets_json,spent_json,created_at,updated_at)
         VALUES ('run','demo',?1,'t','p','/repo','/rd','wf/x','sha','paused','question','{}','{}',0,0)",
        [spec_json],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
         VALUES ('exec-1','run','s1',1,0,'running','verdict','agent-1')",
        [],
    )
    .unwrap();
    (conn, "run".to_string(), "exec-1".to_string())
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn report_persists_and_journals() {
    let (conn, _run, exec) = seed(vec![CommsCap::Report]);
    let (resp, poke) = route(
        &conn,
        None,
        "req-1",
        "run",
        "agent-1",
        "wf_report",
        &json!({ "status": "progress", "note": "halfway" }),
    );
    assert!(resp.ok, "{resp:?}");
    assert!(matches!(poke, Poke::None));
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM wf_message WHERE kind='report'"),
        1
    );
    // Journaled as message_routed against the sending attempt.
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM wf_event WHERE type='message_routed'"
        ),
        1
    );
    let se: String = conn
        .query_row(
            "SELECT step_exec_id FROM wf_event WHERE type='message_routed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(se, exec);
}

#[test]
fn report_without_cap_is_rejected_and_journaled_denied() {
    let (conn, _run, _exec) = seed(vec![]); // no caps
    let (resp, poke) = route(
        &conn,
        None,
        "req-1",
        "run",
        "agent-1",
        "wf_report",
        &json!({ "note": "x" }),
    );
    assert!(!resp.ok);
    assert!(matches!(poke, Poke::None));
    // No message persisted, but the denial is journaled — never silent.
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM wf_message"), 0);
    let denied: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM wf_event WHERE type='message_routed'
             AND json_extract(payload_json,'$.kind')='denied'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(denied, 1);
}

#[test]
fn ask_queues_message_and_reports_poke() {
    let (conn, run, _exec) = seed(vec![CommsCap::Ask]);
    let (resp, poke) = route(
        &conn,
        None,
        "req-1",
        "run",
        "agent-1",
        "wf_ask",
        &json!({ "question": "which db?" }),
    );
    assert!(resp.ok);
    match poke {
        Poke::AskQueued { run_id } => assert_eq!(run_id, run),
        Poke::None => panic!("ask should request a poke"),
    }
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM wf_message WHERE kind='ask' AND status='queued'"
        ),
        1
    );
}

#[test]
fn empty_question_is_rejected() {
    let (conn, _run, _exec) = seed(vec![CommsCap::Ask]);
    let (resp, poke) = route(
        &conn,
        None,
        "req-1",
        "run",
        "agent-1",
        "wf_ask",
        &json!({ "question": "   " }),
    );
    assert!(!resp.ok);
    assert!(matches!(poke, Poke::None));
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM wf_message"), 0);
}

#[test]
fn answer_queues_reply_and_marks_ask_answered() {
    let (conn, run, exec) = seed(vec![CommsCap::Ask]);
    // The step asked a question.
    let (resp, _poke) = route(
        &conn,
        None,
        "req-1",
        "run",
        "agent-1",
        "wf_ask",
        &json!({ "question": "which db?" }),
    );
    let ask_id = resp.stdout.clone().unwrap();

    // The human answers.
    deliver_answer(&conn, None, "p", &run, &ask_id, "Postgres").unwrap();

    // The ask is answered; a queued answer targets the asking attempt.
    let ask_status: String = conn
        .query_row(
            "SELECT status FROM wf_message WHERE id=?1",
            [&ask_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ask_status, "answered");
    let (to, status): (String, String) = conn
        .query_row(
            "SELECT to_step_exec_id, status FROM wf_message WHERE kind='answer'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(to, exec);
    assert_eq!(status, "queued");

    // The queued answer is picked up for the step and coalesced.
    let pending = take_pending_deliveries(&conn, &run, "s1");
    assert_eq!(pending.len(), 1);
    assert!(compose_delivery(&pending).contains("Postgres"));
    // …and marked delivered, so it isn't folded twice.
    assert!(take_pending_deliveries(&conn, &run, "s1").is_empty());
}

#[test]
fn answer_rejects_when_run_not_awaiting() {
    let (conn, run, _exec) = seed(vec![CommsCap::Ask]);
    // No ask outstanding, and we'll flip the run to running.
    conn.execute(
        "UPDATE wf_run SET status='running', paused_reason=NULL WHERE id='run'",
        [],
    )
    .unwrap();
    let err = deliver_answer(&conn, None, "p", &run, "nope", "x");
    assert!(err.is_err());
}

#[test]
fn answer_rejects_a_run_outside_the_project() {
    let (conn, run, _exec) = seed(vec![CommsCap::Ask]);
    let (resp, _) = route(
        &conn,
        None,
        "req-1",
        "run",
        "agent-1",
        "wf_ask",
        &json!({ "question": "q" }),
    );
    let ask_id = resp.stdout.unwrap();
    // The run belongs to project "p"; a caller scoped to another project
    // cannot answer it, and nothing is enqueued.
    let err = deliver_answer(&conn, None, "other", &run, &ask_id, "x");
    assert!(err.is_err(), "answer must be scoped to the run's project");
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM wf_message WHERE kind='answer'"),
        0
    );
    // The correct project succeeds.
    deliver_answer(&conn, None, "p", &run, &ask_id, "x").unwrap();
}

#[test]
fn resolves_sender_before_agent_id_is_stamped() {
    // The scheduler stamps `wf_step_exec.agent_id` only after the turn ends,
    // but comms ops fire *during* the turn. Resolution must work off the
    // run's live attempt while that column is still NULL — otherwise every
    // mid-turn wf_ask/wf_report would fail with "no workflow step".
    let (conn, _run, exec) = seed(vec![CommsCap::Ask]);
    conn.execute(
        "UPDATE wf_step_exec SET agent_id = NULL WHERE id = ?1",
        [&exec],
    )
    .unwrap();
    let (resp, poke) = route(
        &conn,
        None,
        "req-1",
        "run",
        "agent-1",
        "wf_ask",
        &json!({ "question": "which db?" }),
    );
    assert!(
        resp.ok,
        "must resolve by run while agent_id is NULL: {resp:?}"
    );
    assert!(matches!(poke, Poke::AskQueued { .. }));
}

#[test]
fn concurrent_live_attempts_are_not_misattributed() {
    // Parallel comms is unsupported in v1: with two in-flight attempts and no
    // agent_id link yet, the router refuses rather than guess a sender.
    let (conn, _run, _exec) = seed(vec![CommsCap::Ask]);
    conn.execute(
        "UPDATE wf_step_exec SET agent_id = NULL WHERE id = 'exec-1'",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode)
         VALUES ('exec-2','run','s1',1,0,'running','verdict')",
        [],
    )
    .unwrap();
    let (resp, _poke) = route(
        &conn,
        None,
        "req-1",
        "run",
        "agent-1",
        "wf_ask",
        &json!({ "question": "q" }),
    );
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("concurrent"));
}

#[test]
fn ask_is_rejected_once_the_exec_has_committed() {
    // The other half of the commit-point serialization (§10.4): if the
    // scheduler finalized the attempt first (its exec is terminal), a late
    // ask from that turn is rejected — never queued against a completed step,
    // so the run can't be left advanced with an orphan question.
    let (conn, _run, exec) = seed(vec![CommsCap::Ask]);
    conn.execute(
        "UPDATE wf_step_exec SET status = 'done' WHERE id = ?1",
        [&exec],
    )
    .unwrap();
    let (resp, poke) = route(
        &conn,
        None,
        "req-1",
        "run",
        "agent-1",
        "wf_ask",
        &json!({ "question": "q" }),
    );
    assert!(!resp.ok, "ask on a committed exec must be rejected");
    assert!(matches!(poke, Poke::None));
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM wf_message"),
        0,
        "no orphan ask is queued"
    );
}

#[test]
fn has_unanswered_ask_tracks_queued_then_answered() {
    let (conn, run, exec) = seed(vec![CommsCap::Ask]);
    assert!(!has_unanswered_ask(&conn, &exec), "no ask yet");
    let (resp, _) = route(
        &conn,
        None,
        "req-1",
        "run",
        "agent-1",
        "wf_ask",
        &json!({ "question": "q" }),
    );
    let ask_id = resp.stdout.unwrap();
    assert!(has_unanswered_ask(&conn, &exec), "queued ask is pending");
    deliver_answer(&conn, None, "p", &run, &ask_id, "yes").unwrap();
    assert!(
        !has_unanswered_ask(&conn, &exec),
        "answered ask is no longer pending"
    );
}

// ── orchestrator role + decisions (spec §10.2) ────────────────────────

/// A running orchestrate stage: an `orchestrate-0` block (agent `orch`,
/// dynamic `coder` children max 2, child caps `[report, ask]`), one live
/// orchestrator exec, and one live child exec. Returns (conn, run, orch_exec,
/// child_exec).
fn seed_orchestrate() -> (Connection, String, String, String) {
    let td = tempfile::tempdir().unwrap();
    let db = crate::database::init(td.path()).unwrap();
    std::mem::forget(td);
    let conn = Arc::try_unwrap(db).ok().unwrap().into_inner();

    let mut agents = BTreeMap::new();
    for a in ["orch", "coder"] {
        agents.insert(
            a.to_string(),
            AgentSpec {
                base: "claude".into(),
                model: None,
                effort: None,
                instructions: None,
                skills: vec![],
                mcp_servers: vec![],
                custom_agent: None,
            },
        );
    }
    let spec = Spec {
        version: 1,
        name: "demo".into(),
        description: None,
        budgets: None,
        agents,
        workflow: vec![Block::Orchestrate(Orchestrate {
            agent: "orch".into(),
            goal: "lead".into(),
            children: Some(ChildTemplate {
                agent: "coder".into(),
                max: 2,
            }),
            body: vec![],
            join: Join::All,
            integrate: Integrate::None,
            comms: vec![CommsCap::Report, CommsCap::Ask],
            compose: None,
        })],
        finalize: None,
    };
    let spec_json = serde_json::to_string(&spec).unwrap();
    conn.execute(
        "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
            base_sha,status,budgets_json,spent_json,created_at,updated_at)
         VALUES ('run','demo',?1,'t','p','/repo','/rd','wf/x','sha','running','{}','{}',0,0)",
        [spec_json],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
         VALUES ('orch-exec','run','orchestrate-0',1,0,'running','verdict','orch-agent')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
         VALUES ('child-exec','run','child-1',1,0,'running','verdict','child-agent')",
        [],
    )
    .unwrap();
    (
        conn,
        "run".to_string(),
        "orch-exec".to_string(),
        "child-exec".to_string(),
    )
}

#[test]
fn wf_decide_is_orchestrator_only() {
    let (conn, _run, _orch, _child) = seed_orchestrate();
    let (resp, poke) = route(
        &conn,
        None,
        "r1",
        "run",
        "child-agent",
        "wf_decide",
        &json!({ "decision": "stage_done" }),
    );
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("orchestrator"));
    assert!(matches!(poke, Poke::None));
}

#[test]
fn child_caps_come_from_the_orchestrate_block() {
    let (conn, run, _orch, _child) = seed_orchestrate();
    let spec = load_spec(&conn, &run).unwrap();
    // Child inherits the block's [report, ask]; the orchestrator gets all.
    let child_caps = resolve_caps(&conn, &spec, &run, "child-1").unwrap();
    assert_eq!(child_caps, vec![CommsCap::Report, CommsCap::Ask]);
    let orch_caps = resolve_caps(&conn, &spec, &run, "orchestrate-0").unwrap();
    assert!(
        orch_caps.contains(&CommsCap::Notify),
        "orchestrator gets notify"
    );
}

#[test]
fn child_ask_routes_to_the_orchestrator_not_the_human() {
    let (conn, _run, orch, child) = seed_orchestrate();
    let (resp, poke) = route(
        &conn,
        None,
        "r1",
        "run",
        "child-agent",
        "wf_ask",
        &json!({ "question": "which db?" }),
    );
    assert!(resp.ok, "{resp:?}");
    // No human pause — the orchestrator handles it.
    assert!(
        matches!(poke, Poke::None),
        "child ask must not pause the run"
    );
    let to: String = conn
        .query_row(
            "SELECT to_step_exec_id FROM wf_message WHERE kind='ask'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(to, orch);
    assert!(
        has_unanswered_ask(&conn, &child),
        "child defers until answered"
    );
}

#[test]
fn orchestrator_answers_a_child_ask() {
    let (conn, run, orch, child) = seed_orchestrate();
    // The child asks.
    let (resp, _) = route(
        &conn,
        None,
        "r1",
        "run",
        "child-agent",
        "wf_ask",
        &json!({ "question": "which db?", "options": ["pg", "sqlite"] }),
    );
    let ask_id = resp.stdout.unwrap();

    // The orchestrator sees it in its inbox with the message id to answer.
    let inbox = take_orchestrator_inbox(&conn, &run, &orch);
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].from_step_id, "child-1");
    assert_eq!(inbox[0].message.id, ask_id);
    assert!(compose_orchestrator_inbox(&inbox).contains(&ask_id));

    // The orchestrator answers via wf_decide.
    let (resp2, poke2) = route(
        &conn,
        None,
        "r2",
        "run",
        "orch-agent",
        "wf_decide",
        &json!({ "decision": "answer", "message_id": ask_id, "body": "use Postgres" }),
    );
    assert!(resp2.ok, "{resp2:?}");
    assert!(matches!(poke2, Poke::None));

    // The child is no longer waiting; the answer is queued for its next turn.
    assert!(!has_unanswered_ask(&conn, &child));
    let pending = take_pending_deliveries(&conn, &run, "child-1");
    assert_eq!(pending.len(), 1);
    assert!(compose_delivery(&pending).contains("use Postgres"));

    // The decision is journaled.
    let decided: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM wf_event WHERE type='decision'
             AND json_extract(payload_json,'$.decision')='answer'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(decided, 1);
}

#[test]
fn spawn_child_is_bounded_by_children_max_and_denials_journal() {
    let (conn, run, orch, _child) = seed_orchestrate(); // children.max = 2
    for i in 0..2 {
        let (resp, _) = route(
            &conn,
            None,
            &format!("s{i}"),
            "run",
            "orch-agent",
            "wf_decide",
            &json!({ "decision": "spawn_child", "agent": "coder", "goal": "a slice" }),
        );
        assert!(resp.ok, "spawn {i} should be approved: {resp:?}");
    }
    // The third exceeds children.max → structured error + child_spawn_denied.
    let (resp3, poke3) = route(
        &conn,
        None,
        "s3",
        "run",
        "orch-agent",
        "wf_decide",
        &json!({ "decision": "spawn_child", "agent": "coder", "goal": "one too many" }),
    );
    assert!(!resp3.ok);
    assert!(resp3.error.unwrap().contains("children.max"));
    assert!(matches!(poke3, Poke::None));
    let denied: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM wf_event WHERE type='child_spawn_denied'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(denied, 1);
    let approved: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM wf_event WHERE type='child_spawn_approved'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(approved, 2);

    // The two approvals are consumable by the scheduler as SpawnChild decisions.
    let decisions = take_orchestrator_decisions(&conn, &run, &orch);
    assert_eq!(decisions.len(), 2);
    assert!(decisions
        .iter()
        .all(|d| matches!(d, Decision::SpawnChild { .. })));
}

#[test]
fn spawn_child_agent_must_match_the_template() {
    let (conn, _run, _orch, _child) = seed_orchestrate();
    let (resp, _) = route(
        &conn,
        None,
        "s1",
        "run",
        "orch-agent",
        "wf_decide",
        &json!({ "decision": "spawn_child", "agent": "orch", "goal": "wrong agent" }),
    );
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("template's child agent"));
}

#[test]
fn skip_and_retry_child_reject_steps_outside_the_declared_namespace() {
    // §10.2: an unknown-step decision must return a structured error, not be
    // queued and silently dropped by the stage. Validated against the
    // definition-declared child namespace (static body ids + `::dyn-<k>`
    // within `children.max`), which is race-free vs. the child's own spawn.
    let (conn, run, orch, _child) = seed_orchestrate(); // body: [], children.max = 2

    // A dynamic child within the template bound is a valid target.
    let (ok, _) = route(
        &conn,
        None,
        "d0",
        "run",
        "orch-agent",
        "wf_decide",
        &json!({ "decision": "retry_child", "step_id": "orchestrate-0::dyn-0", "guidance": "again" }),
    );
    assert!(ok.ok, "a declared dynamic child must be accepted: {ok:?}");

    // A dyn index at/over children.max, an undeclared id, and the
    // orchestrator's own step are all rejected with a structured error.
    for (label, step_id) in [
        ("over-max", "orchestrate-0::dyn-2"),
        ("unknown", "ghost"),
        ("self", "orchestrate-0"),
    ] {
        let (resp, poke) = route(
            &conn,
            None,
            label,
            "run",
            "orch-agent",
            "wf_decide",
            &json!({ "decision": "skip_child", "step_id": step_id, "reason": "x" }),
        );
        assert!(!resp.ok, "{label} ({step_id}) must be rejected");
        assert!(
            resp.error.unwrap().contains("unknown child step"),
            "{label} must be a structured unknown-step error"
        );
        assert!(matches!(poke, Poke::None));
    }

    // Only the one valid decision was queued for the stage to consume.
    let decisions = take_orchestrator_decisions(&conn, &run, &orch);
    assert_eq!(decisions.len(), 1);
    assert!(matches!(decisions[0], Decision::RetryChild { .. }));
}

#[test]
fn notify_is_orchestrator_only_and_reaches_children() {
    let (conn, run, _orch, _child) = seed_orchestrate();
    // The orchestrator notifies the child.
    let (resp, _) = route(
        &conn,
        None,
        "n1",
        "run",
        "orch-agent",
        "wf_notify",
        &json!({ "to": "child-1", "message": "slice B landed" }),
    );
    assert!(resp.ok, "{resp:?}");
    let pending = take_pending_deliveries(&conn, &run, "child-1");
    assert_eq!(pending.len(), 1);
    assert!(compose_delivery(&pending).contains("slice B landed"));

    // A child cannot notify (its caps are [report, ask]).
    let (resp2, _) = route(
        &conn,
        None,
        "n2",
        "run",
        "child-agent",
        "wf_notify",
        &json!({ "to": "all-children", "message": "x" }),
    );
    assert!(!resp2.ok);
}

#[test]
fn lifecycle_is_auto_forwarded_to_the_orchestrator() {
    let (conn, run, orch, child) = seed_orchestrate();
    forward_lifecycle(
        &conn,
        None,
        &run,
        &orch,
        &child,
        "done",
        "child `child-1` finished",
    );
    let inbox = take_orchestrator_inbox(&conn, &run, &orch);
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].from_step_id, "child-1");
    let rendered = compose_orchestrator_inbox(&inbox);
    assert!(
        rendered.contains("child-1") && rendered.contains("finished"),
        "{rendered}"
    );
    // Shown once — not re-delivered on the next turn.
    assert!(take_orchestrator_inbox(&conn, &run, &orch).is_empty());
}

#[test]
fn stage_done_and_escalate_decisions() {
    let (conn, run, orch, _child) = seed_orchestrate();
    // stage_done queues a consumable decision.
    let (resp, _) = route(
        &conn,
        None,
        "d1",
        "run",
        "orch-agent",
        "wf_decide",
        &json!({ "decision": "stage_done" }),
    );
    assert!(resp.ok, "{resp:?}");
    // escalate queues an ask to the human and pauses the run.
    let (resp2, poke2) = route(
        &conn,
        None,
        "d2",
        "run",
        "orch-agent",
        "wf_decide",
        &json!({ "decision": "escalate", "question": "which framework?" }),
    );
    assert!(resp2.ok, "{resp2:?}");
    assert!(
        matches!(poke2, Poke::AskQueued { .. }),
        "escalate pauses for the human"
    );
    // The escalation appears as an unanswered ask from the orchestrator.
    assert!(has_unanswered_ask(&conn, &orch));

    let decisions = take_orchestrator_decisions(&conn, &run, &orch);
    assert_eq!(decisions, vec![Decision::StageDone]);
}

#[test]
fn spawn_limit_persists_across_resume() {
    let (conn, _run, _orch, _child) = seed_orchestrate(); // children.max = 2
    for i in 0..2 {
        let (resp, _) = route(
            &conn,
            None,
            &format!("s{i}"),
            "run",
            "orch-agent",
            "wf_decide",
            &json!({ "decision": "spawn_child", "agent": "coder", "goal": "slice" }),
        );
        assert!(resp.ok, "{resp:?}");
    }
    // Resume: the stage gets a fresh orchestrator exec (same `orchestrate-0`
    // step id); the old one is no longer live.
    conn.execute(
        "UPDATE wf_step_exec SET status='abandoned' WHERE id='orch-exec'",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
         VALUES ('orch-exec-2','run','orchestrate-0',2,0,'running','verdict','orch-agent-2')",
        [],
    )
    .unwrap();
    // The resumed orchestrator cannot re-grant a whole new batch — the count is
    // stage-wide, not per-exec.
    let (resp, _) = route(
        &conn,
        None,
        "s3",
        "run",
        "orch-agent-2",
        "wf_decide",
        &json!({ "decision": "spawn_child", "agent": "coder", "goal": "one too many" }),
    );
    assert!(!resp.ok, "spawn limit must persist across resume: {resp:?}");
    assert!(resp.error.unwrap().contains("children.max"));
}

// ── dynamic composition, wf_compose (spec §10.3) ──────────────────────

/// A running orchestrate stage with `compose` enabled. `comms` are the stage's
/// children caps; `turns` seeds the run budget; `parent` sets `parent_run_id`
/// (drives the depth check). Returns (conn, orch_exec).
fn seed_compose(
    limits: Option<ComposeLimits>,
    comms: Vec<CommsCap>,
    turns: i64,
    parent: Option<&str>,
) -> (Connection, String) {
    let td = tempfile::tempdir().unwrap();
    let db = crate::database::init(td.path()).unwrap();
    std::mem::forget(td);
    let conn = Arc::try_unwrap(db).ok().unwrap().into_inner();

    let mut agents = BTreeMap::new();
    for a in ["orch", "coder"] {
        agents.insert(
            a.to_string(),
            AgentSpec {
                base: "claude".into(),
                model: None,
                effort: None,
                instructions: None,
                skills: vec![],
                mcp_servers: vec![],
                custom_agent: None,
            },
        );
    }
    let spec = Spec {
        version: 1,
        name: "demo".into(),
        description: None,
        budgets: None,
        agents,
        workflow: vec![Block::Orchestrate(Orchestrate {
            agent: "orch".into(),
            goal: "lead".into(),
            children: Some(ChildTemplate {
                agent: "coder".into(),
                max: 2,
            }),
            body: vec![],
            join: Join::All,
            integrate: Integrate::None,
            comms,
            compose: limits,
        })],
        finalize: None,
    };
    let spec_json = serde_json::to_string(&spec).unwrap();
    let budgets_json = serde_json::to_string(&crate::workflow::budget::EffectiveBudgets {
        turns,
        ..Default::default()
    })
    .unwrap();
    // Satisfy the parent_run_id FK when the run is itself a sub-run.
    if let Some(p) = parent {
        conn.execute(
            "INSERT INTO wf_run (id,name,spec_json,task,project_id,repo_path,run_dir,branch,
                base_sha,status,budgets_json,spent_json,created_at,updated_at)
             VALUES (?1,'p','{}','t','p','/repo','/rd','wf/p','sha','running','{}','{}',0,0)",
            [p],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO wf_run (id,parent_run_id,name,spec_json,task,project_id,repo_path,run_dir,
            branch,base_sha,status,budgets_json,spent_json,created_at,updated_at)
         VALUES ('run',?1,'demo',?2,'t','p','/repo','/rd','wf/x','sha','running',?3,'{}',0,0)",
        rusqlite::params![parent, spec_json, budgets_json],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
         VALUES ('orch-exec','run','orchestrate-0',1,0,'running','verdict','orch-agent')",
        [],
    )
    .unwrap();
    (conn, "orch-exec".to_string())
}

/// A minimal one-step fragment whose step declares `caps` and uses `agent`.
fn fragment(caps: Vec<&str>, agent: &str) -> Value {
    json!([{ "step": { "id": "impl", "agent": agent, "goal": "do it", "comms": caps } }])
}

fn compose_args(fragment: Value, turns: i64) -> Value {
    json!({
        "task": "a composed slice",
        "fragment": fragment,
        "budgets": { "turns": turns },
        "integrate": "merge",
        "base": "parent-head",
    })
}

fn compose(conn: &Connection, agent_id: &str, args: &Value) -> Response {
    route(conn, None, "c1", "run", agent_id, "wf_compose", args).0
}

#[test]
fn wf_compose_is_orchestrator_only() {
    let (conn, _orch) = seed_compose(
        Some(ComposeLimits {
            max_sub_runs: 2,
            max_depth: 2,
        }),
        vec![CommsCap::Report],
        100,
        None,
    );
    // Add a non-orchestrator child exec and send as it. (Its step id must not
    // start with the orchestrate prefix, which marks the orchestrator role.)
    conn.execute(
        "INSERT INTO wf_step_exec (id,run_id,step_id,attempt,iteration,status,gate_mode,agent_id)
         VALUES ('child-exec','run','child-1',1,0,'running','verdict','child-agent')",
        [],
    )
    .unwrap();
    let resp = compose(
        &conn,
        "child-agent",
        &compose_args(fragment(vec![], "coder"), 10),
    );
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("orchestrator only"));
}

#[test]
fn wf_compose_denied_when_composition_disabled() {
    let (conn, _orch) = seed_compose(None, vec![CommsCap::Report], 100, None);
    let resp = compose(
        &conn,
        "orch-agent",
        &compose_args(fragment(vec![], "coder"), 10),
    );
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("not enabled"));
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM wf_event WHERE type='compose_denied'"
        ),
        1,
        "a rejection is always journaled"
    );
}

#[test]
fn wf_compose_valid_request_queues_a_decision() {
    let (conn, _orch) = seed_compose(
        Some(ComposeLimits {
            max_sub_runs: 2,
            max_depth: 2,
        }),
        vec![CommsCap::Report, CommsCap::Ask],
        100,
        None,
    );
    let resp = compose(
        &conn,
        "orch-agent",
        &compose_args(fragment(vec!["report"], "coder"), 30),
    );
    assert!(resp.ok, "valid compose should be accepted: {resp:?}");
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM wf_message WHERE kind='decision'
               AND json_extract(body_json,'$.decision')='compose' AND status='queued'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM wf_event WHERE type='compose_requested'"
        ),
        1
    );
    // The scheduler decodes it into a typed Compose decision.
    let decisions = take_orchestrator_decisions(&conn, "run", "orch-exec");
    assert_eq!(decisions.len(), 1);
    assert!(matches!(decisions[0], Decision::Compose(_)));
}

#[test]
fn wf_compose_rejects_over_budget() {
    // Run turn cap is 20; a 50-turn slice can't fit.
    let (conn, _orch) = seed_compose(
        Some(ComposeLimits {
            max_sub_runs: 2,
            max_depth: 2,
        }),
        vec![CommsCap::Report],
        20,
        None,
    );
    let resp = compose(
        &conn,
        "orch-agent",
        &compose_args(fragment(vec![], "coder"), 50),
    );
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("remain in the run budget"));
}

#[test]
fn wf_compose_rejects_over_depth() {
    // The run is itself a sub-run (parent set → depth 1); with max_depth 1 a
    // further sub-run would be depth 2.
    let (conn, _orch) = seed_compose(
        Some(ComposeLimits {
            max_sub_runs: 2,
            max_depth: 1,
        }),
        vec![CommsCap::Report],
        100,
        Some("parent-run"),
    );
    let resp = compose(
        &conn,
        "orch-agent",
        &compose_args(fragment(vec![], "coder"), 10),
    );
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("depth"));
}

#[test]
fn wf_compose_rejects_caps_escalation() {
    // Stage grants children only `report`; a fragment step wanting `ask` is a
    // privilege escalation (spec §15).
    let (conn, _orch) = seed_compose(
        Some(ComposeLimits {
            max_sub_runs: 2,
            max_depth: 2,
        }),
        vec![CommsCap::Report],
        100,
        None,
    );
    let resp = compose(
        &conn,
        "orch-agent",
        &compose_args(fragment(vec!["ask"], "coder"), 10),
    );
    assert!(!resp.ok);
    assert!(resp
        .error
        .unwrap()
        .contains("broader than the stage grants"));
}

#[test]
fn wf_compose_rejects_invalid_fragment() {
    let (conn, _orch) = seed_compose(
        Some(ComposeLimits {
            max_sub_runs: 2,
            max_depth: 2,
        }),
        vec![CommsCap::Report],
        100,
        None,
    );
    // References an agent that isn't in the (inherited) agent map.
    let resp = compose(
        &conn,
        "orch-agent",
        &compose_args(fragment(vec![], "nonexistent"), 10),
    );
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("fragment is invalid"));
}

#[test]
fn wf_compose_rejects_over_max_sub_runs() {
    let (conn, _orch) = seed_compose(
        Some(ComposeLimits {
            max_sub_runs: 1,
            max_depth: 2,
        }),
        vec![CommsCap::Report],
        100,
        None,
    );
    // One sub-run already exists for this parent.
    conn.execute(
        "INSERT INTO wf_run (id,parent_run_id,name,spec_json,task,project_id,repo_path,run_dir,
            branch,base_sha,status,budgets_json,spent_json,created_at,updated_at)
         VALUES ('sub-1','run','s','{}','t','p','/repo','/rd','wf/s','sha','running','{}','{}',0,0)",
        [],
    )
    .unwrap();
    let resp = compose(
        &conn,
        "orch-agent",
        &compose_args(fragment(vec![], "coder"), 10),
    );
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("max_sub_runs"));
}
