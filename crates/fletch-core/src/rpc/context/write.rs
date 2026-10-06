//! The three writes. An entity lands as soon as its slug is free; a decision
//! is first classified against the heads about the same entities, and lands
//! only when it is new or the agent has said how it relates to what is there.

use serde_json::{json, Value};

use crate::context::{
    resolve, trust, Assertion, AssertionInput, AssertionKind, AssertionStatus, Contradict,
    EntityInput, Graph, LinkChange, RelationKind, Stamp, Stance,
};
use crate::rpc::Response;

use super::args::{
    clean_list, explain, parse_required, reply, text, LinkArgs, RecordDecisionArgs,
    RecordEntityArgs, RelatesArg, MAX_NAME, MAX_RATIONALE, MAX_STATEMENT, MAX_SUMMARY,
};
use super::ContextDispatcher;

const HINT: &str = "current decisions about these entities exist; resubmit with \
                    supersedes: {id, reasoning} to replace one, contradicts: [id] to \
                    record a tension, or coexists: true when they all hold";

impl ContextDispatcher {
    pub(super) async fn record_entity(&self, id: &str, args: &Value) -> Response {
        let result = match entity_input(args) {
            Ok((input, relates)) => {
                let stamp = self.stamp(None).await;
                self.write_entity(input, relates, stamp)
            }
            Err(e) => Err(e),
        };
        reply(id, "context_record_entity", result)
    }

    pub(super) async fn record_decision(&self, id: &str, args: &Value) -> Response {
        let result = match parse_required::<RecordDecisionArgs>(args) {
            Ok(a) => match self.verify_user_quote(a.user_quote.as_deref()) {
                Ok(user) => {
                    let stamp = self.stamp(user.as_ref()).await;
                    self.write_decision(a, user.is_some(), stamp)
                }
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        };
        reply(id, "context_record_decision", result)
    }

    pub(super) async fn link(&self, id: &str, args: &Value) -> Response {
        let result = match parse_required::<LinkArgs>(args) {
            Ok(a) => {
                let stamp = self.stamp(None).await;
                self.write_link(a, stamp)
            }
            Err(e) => Err(e),
        };
        reply(id, "context_link", result)
    }

    fn graph(&self) -> Result<Graph, String> {
        self.store.load(&self.project_id).map_err(|e| e.to_string())
    }

    /// The entity is recorded before its relations are resolved, so a bad
    /// `relates.to` reports what did land rather than losing the entity.
    fn write_entity(
        &self,
        input: EntityInput,
        relates: Vec<RelatesArg>,
        stamp: Stamp,
    ) -> Result<Value, String> {
        let slug = input.slug.clone();
        let id = self
            .store
            .record_entity(&self.project_id, input, stamp.clone())
            .map_err(explain)?;
        if relates.is_empty() {
            return Ok(json!({ "id": id, "slug": slug }));
        }
        let graph = self.graph()?;
        let failed: Vec<String> = relates
            .into_iter()
            .filter_map(|r| {
                resolve::entity(&graph, &r.to)
                    .map_err(explain)
                    .and_then(|to| {
                        let change = LinkChange {
                            from: id.clone(),
                            to: to.id.clone(),
                            rel: r.rel,
                            add: true,
                        };
                        self.store
                            .link(&self.project_id, change, stamp.clone())
                            .map_err(explain)
                    })
                    .err()
            })
            .collect();
        if failed.is_empty() {
            Ok(json!({ "id": id, "slug": slug }))
        } else {
            Err(format!(
                "recorded `{slug}` as {id}, but could not link it: {}",
                failed.join("; ")
            ))
        }
    }

    /// A `user_quote` must be found verbatim in the user's own turns; the
    /// claim is never taken from the caller (`context::trust`).
    fn verify_user_quote(&self, quote: Option<&str>) -> Result<Option<trust::UserStated>, String> {
        let Some(quote) = quote.map(str::trim).filter(|q| !q.is_empty()) else {
            return Ok(None);
        };
        let turns = self.user_turns()?;
        match trust::find_user_quote(&turns, quote) {
            Some(found) => Ok(Some(found)),
            None => Err(format!(
                "`user_quote` was not found in the user's messages of this workspace (quote at \
                 least {} characters of what they wrote, verbatim). Leave it out to record this \
                 as your own, provisional, statement.",
                trust::MIN_QUOTE_CHARS
            )),
        }
    }

    fn write_decision(
        &self,
        a: RecordDecisionArgs,
        user_stated: bool,
        stamp: Stamp,
    ) -> Result<Value, String> {
        let statement = text("statement", &a.statement, MAX_STATEMENT)?;
        let rationale = text("rationale", &a.rationale, MAX_RATIONALE)?;
        let about = clean_list(&a.about);
        if about.is_empty() {
            return Err("`about` must name at least one entity (a slug from the index)".into());
        }
        let graph = self.graph()?;
        let (found, unknown) = resolve::entities(&graph, &about);
        if !unknown.is_empty() {
            return Err(format!(
                "unknown entities in `about`: {} — record each with context_record_entity \
                 first, or use a slug from the index",
                unknown.join(", ")
            ));
        }
        let mut about: Vec<String> = found.iter().map(|e| e.id.clone()).collect();
        about.dedup();

        let explicit = a.supersedes.is_some() || !a.contradicts.is_empty() || a.coexists;
        let input = AssertionInput {
            kind: a.kind.unwrap_or(AssertionKind::Decision),
            domain: a.domain,
            stance: a.stance.unwrap_or(Stance::Adopted),
            statement,
            rationale,
            valid_from: None,
            paths: clean_list(&a.paths),
            about,
            supersedes: a.supersedes,
            contradicts: a
                .contradicts
                .into_iter()
                .map(|id| Contradict {
                    id,
                    reasoning: None,
                })
                .collect(),
            status: if user_stated {
                AssertionStatus::Confirmed
            } else {
                AssertionStatus::Provisional
            },
        };

        let relation = resolve::classify(&graph, &input);
        if relation.kind == RelationKind::Duplicate {
            return Ok(json!({ "already_recorded": relation.target }));
        }
        if !explicit {
            // Whether the new statement replaces, contradicts or joins what
            // is already said about these entities is the agent's call, made
            // once it has seen the heads.
            let related = resolve::related_heads(&graph, &input);
            if !related.is_empty() {
                return Ok(conflict(&related));
            }
        }
        let status = input.status;
        let id = self
            .store
            .record_assertion(&self.project_id, input, stamp)
            .map_err(explain)?;
        Ok(json!({ "id": id, "status": status }))
    }

    fn write_link(&self, a: LinkArgs, stamp: Stamp) -> Result<Value, String> {
        let graph = self.graph()?;
        let from = resolve::entity(&graph, &a.from).map_err(explain)?;
        let to = resolve::entity(&graph, &a.to).map_err(explain)?;
        let change = LinkChange {
            from: from.id.clone(),
            to: to.id.clone(),
            rel: a.rel,
            add: !a.remove,
        };
        self.store
            .link(&self.project_id, change, stamp)
            .map_err(explain)?;
        Ok(json!({ "from": from.slug, "to": to.slug, "rel": a.rel, "linked": !a.remove }))
    }
}

fn entity_input(args: &Value) -> Result<(EntityInput, Vec<RelatesArg>), String> {
    let a: RecordEntityArgs = parse_required(args)?;
    let slug = text("slug", &a.slug, MAX_NAME)?;
    if slug.chars().any(char::is_whitespace) {
        return Err("`slug` is one token, like `billing-ui`".into());
    }
    let input = EntityInput {
        id: a.id,
        slug,
        kind: a.kind.unwrap_or_default(),
        name: text("name", &a.name, MAX_NAME)?,
        summary: text("summary", &a.summary, MAX_SUMMARY)?,
        aliases: clean_list(&a.aliases),
        paths: clean_list(&a.paths),
    };
    Ok((input, a.relates))
}

/// The first step of the two-step protocol: nothing written, the heads the
/// statement sits next to named.
fn conflict(related: &[&Assertion]) -> Value {
    let heads: Vec<Value> = related.iter().map(|a| head(a)).collect();
    json!({ "conflict": "related", "heads": heads, "hint": HINT })
}

fn head(a: &Assertion) -> Value {
    json!({
        "id": a.id,
        "kind": a.kind,
        "domain": a.domain,
        "stance": a.stance,
        "statement": a.statement,
        "rationale": a.rationale,
        "author_kind": a.author.kind,
        "source_kind": a.source.kind,
    })
}
