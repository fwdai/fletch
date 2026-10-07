use super::*;

use serde_json::{json, Value};
use std::sync::Mutex;

pub(super) const PROJECT: &str = "ctx-p1";
pub(super) const FLETCH_PROJECT: &str = "p1";
pub(super) const AGENT: &str = "agent-1";

/// Answers `ping` and nothing else: enough to prove delegation.
pub(super) struct Inner;

impl RpcDispatcher for Inner {
    fn dispatch<'a>(
        &'a self,
        id: &'a str,
        op: &'a str,
        _args: &'a Value,
    ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
        Box::pin(async move {
            let resp = match op {
                "ping" => Response::ok(id, 0, "pong".to_string(), String::new()),
                other => Response::err(id, format!("unknown op: {other}")),
            };
            (resp, Vec::new())
        })
    }
}

pub(super) const REPO: &str = "quorum";

/// A dispatcher over a fresh store; the temp dir is the (non-git) checkout,
/// so provenance comes back without a branch or commit. The gate is open in
/// `ContextStore::temp`, so the project is built directly.
pub(super) fn dispatcher() -> (ContextDispatcher, tempfile::TempDir) {
    dispatcher_with_repos(vec![REPO.into()])
}

/// A dispatcher for a workspace with these checkouts (primary first).
pub(super) fn dispatcher_with_repos(repos: Vec<String>) -> (ContextDispatcher, tempfile::TempDir) {
    let (dispatcher, dir, _) = dispatcher_with_checkout_state(repos);
    (dispatcher, dir)
}

/// A dispatcher whose live checkout list a test can extend mid-session.
pub(super) fn dispatcher_with_checkout_state(
    repos: Vec<String>,
) -> (
    ContextDispatcher,
    tempfile::TempDir,
    Arc<Mutex<Vec<Checkout>>>,
) {
    let (store, dir) = crate::context::ContextStore::temp().unwrap();
    let db = store.db().clone();
    let checkouts = Arc::new(Mutex::new(
        repos
            .into_iter()
            .map(|repo| Checkout {
                path: dir.path().join(&repo),
                repo,
            })
            .collect::<Vec<_>>(),
    ));
    let state = checkouts.clone();
    let resolver: CheckoutResolver = Arc::new(move || Ok(checkouts.lock().unwrap().clone()));
    let d = ContextDispatcher {
        inner: Arc::new(Inner),
        service: ContextService::new(db.clone(), Arc::new(crate::host::sink::NullSink)).unwrap(),
        project: context::Project {
            id: PROJECT.into(),
            fletch_id: FLETCH_PROJECT.into(),
        },
        agent_id: AGENT.into(),
        provider: "claude".into(),
        checkouts: resolver,
        session_id: Some("sess-1".into()),
        db,
    };
    (d, dir, state)
}

/// The project's graph as the store holds it.
pub(super) fn graph(d: &ContextDispatcher) -> crate::context::Graph {
    d.service.store().load(PROJECT).unwrap()
}

pub(super) async fn call(d: &ContextDispatcher, op: &str, args: Value) -> Response {
    d.dispatch("r1", op, &args).await.0
}

pub(super) fn payload(resp: &Response) -> Value {
    assert!(resp.ok, "{:?}", resp.error);
    serde_json::from_str(resp.stdout.as_ref().unwrap()).unwrap()
}

pub(super) fn error(resp: &Response) -> String {
    assert!(!resp.ok, "expected a refusal, got {:?}", resp.stdout);
    resp.error.clone().unwrap()
}

/// Records a feature entity and returns its id.
pub(super) async fn entity(d: &ContextDispatcher, slug: &str) -> String {
    let resp = call(
        d,
        "context_record_entity",
        json!({ "slug": slug, "kind": "feature", "name": slug, "summary": format!("the {slug} feature") }),
    )
    .await;
    payload(&resp)["id"].as_str().unwrap().to_string()
}

/// An adopted architectural decision about `about`, with the given statement.
pub(super) fn decision(about: &str, statement: &str) -> Value {
    json!({
        "domain": "architectural",
        "statement": statement,
        "rationale": "because",
        "about": [about],
    })
}
