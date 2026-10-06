//! `context_get`: compile the picture for what the agent named and log what
//! was served — the misses in that log are the coverage signal the UI shows.

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
            as_of: None,
            budget_chars: 0,
        };
        let graph = self
            .store
            .load(&self.project_id)
            .map_err(|e| e.to_string())?;
        let bundle = compile::compile(&graph, &query, self.vision_fallback());
        let markdown = render::render_markdown(&bundle);

        let read = ReadRecord {
            project_id: self.project_id.clone(),
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
        if let Err(e) = self.store.log_read(&read) {
            tracing::warn!("context: read not logged for agent {}: {e}", self.agent_id);
        }
        Ok(markdown)
    }

    /// The roadmap brief stands in for the vision until one is recorded. A
    /// read failure is no fallback, not a failed `context_get`.
    fn vision_fallback(&self) -> Option<String> {
        let conn = self.db.lock();
        match crate::roadmap::memory::load(&conn, &self.fletch_project_id) {
            Ok(brief) => brief.map(|b| b.content),
            Err(e) => {
                tracing::warn!("context: roadmap brief unavailable as vision fallback: {e}");
                None
            }
        }
    }
}
