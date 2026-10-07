//! `context_get`: compile the picture for what the agent named and log what
//! was served — the misses in that log are the coverage signal the UI shows.
//! Path anchors are checked against the workspace's primary checkout, so an
//! agent is told when what it is served points at code that is gone.

use serde_json::Value;

use crate::context::{compile, render, CompileQuery, ReadRecord};
use crate::rpc::Response;

use super::args::{clean_list, parse_args, reply_text, GetArgs};
use super::ContextDispatcher;

impl ContextDispatcher {
    pub(super) fn get(&self, id: &str, args: &Value) -> Response {
        reply_text(id, "context_get", self.compile(args))
    }

    fn compile(&self, args: &Value) -> Result<String, String> {
        let a: GetArgs = parse_args(args)?;
        let query = CompileQuery {
            entities: clean_list(&a.entities),
            query: a
                .query
                .map(|q| q.trim().to_string())
                .filter(|q| !q.is_empty()),
            paths: clean_list(&a.paths),
            include_history: a.include_history,
            budget_chars: 0,
            overview: false,
        };
        let store = self.service.store();
        let graph = self
            .service
            .graph(&self.project)
            .map_err(|e| e.to_string())?;
        let checkout = self.primary_checkout().map(|c| c.path);
        let bundle = compile::compile(&graph, &query, checkout.as_deref());
        let markdown = render::render_markdown(&bundle);

        let read = ReadRecord {
            project_id: self.project.id.clone(),
            agent_id: Some(self.agent_id.clone()),
            workspace_id: Some(self.agent_id.clone()),
            session_id: self.session_id.clone(),
            query,
            served_entities: bundle
                .entities
                .iter()
                .map(|e| e.entity.id.clone())
                .collect(),
            served_assertions: bundle
                .assertions
                .iter()
                .map(|a| a.assertion.id.clone())
                .collect(),
            misses: bundle.misses.clone(),
            chars: markdown.chars().count(),
        };
        if let Err(e) = store.log_read(&read) {
            tracing::warn!("context: read not logged for agent {}: {e}", self.agent_id);
        }
        Ok(markdown)
    }
}
