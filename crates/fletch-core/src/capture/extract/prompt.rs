//! The extractor's prompt and the shape of its answer. The model gets the
//! plan, the entity index, the current heads and the new turns, and is asked
//! for strict JSON; the parse is tolerant of fences and prose around it and
//! skips any single item that does not fit, so one bad entry never costs the
//! whole run.

use serde::Deserialize;
use serde_json::Value;

use super::input::ExtractInput;
use crate::context::model::*;

/// Bump when the instructions change in a way that alters what gets
/// proposed; recorded on every run so outputs can be compared across versions.
pub const PROMPT_VERSION: &str = "3";

/// The most a run may propose, as the instructions state; the pipeline
/// drops the rest, so a model that ignores them cannot flood the review.
pub const MAX_ENTITIES: usize = 1;
pub const MAX_ASSERTIONS: usize = 3;

const INSTRUCTIONS: &str = "\
You maintain a project's context graph: the entities a product is made of (vision, goals, \
capabilities, features, modules, topics) and the assertions about them (decisions, constraints, \
facts), each with a domain (business, architectural) and a stance (adopted, rejected). Below \
are the workspace's plan, the entities already known, the current head assertions, and the \
latest turns of a conversation between the user and a coding agent. Read the turns and propose \
what the graph should learn from them.

The bar: propose a statement only if an agent working later, on a different branch, would act \
differently for knowing it.

What counts:
- constraints — rules the work must follow;
- rejected alternatives, with the reason they were rejected;
- architectural decisions that reach beyond the change at hand;
- facts about how the product or the business works.

What does not count:
- decisions local to one change: how a function was written, which file was edited — the \
merged pull request's Decisions section already carries those;
- commands run, test output, progress narration;
- anything already in <heads> unless it changed — then relate the new assertion to it.

Rules:
- Domain is `business` or `architectural`, nothing else; anything that would be \
implementation detail does not count.
- At most 3 assertions and at most 1 new entity. Prefer proposing nothing over proposing \
something marginal.
- Reuse existing slugs from <entities> for `about`. Propose a new entity only when no \
existing one fits; give it a short kebab-case slug.
- Give every assertion at least one `evidence` quote copied verbatim from the turns — the \
user's own words or the agent's; never paraphrase a quote. Quotes are checked against the \
turns: an assertion whose quotes are not found there is dropped, and only a quote from the \
user's own message marks a statement as the user's.
- When a head changed, use relation `supersedes` with the head's `target` id and say why in \
`reasoning`. When a fact disagrees with a recorded decision, use `contradicts` with the id. \
Use `duplicate` when the head already says the same thing; `new` otherwise.
- Keep statements short and declarative; put the why in `rationale`.
- Propose nothing when the turns hold nothing that counts.

Reply with strict JSON only, no prose and no code fence, in exactly this shape:
{\"entities\": [{\"slug\": \"\", \"kind\": \"vision|goal|capability|feature|module|topic\", \
\"name\": \"\", \"summary\": \"\", \"aliases\": [], \"paths\": []}], \
\"assertions\": [{\"about\": [\"slug\"], \"kind\": \"decision|constraint|fact\", \
\"domain\": \"business|architectural\", \"stance\": \"adopted|rejected\", \
\"statement\": \"\", \"rationale\": \"\", \
\"relation\": {\"kind\": \"new|confirms|supersedes|contradicts|duplicate\", \"target\": \"id\", \
\"reasoning\": \"\"}, \"evidence\": [{\"quote\": \"\"}]}]}";

/// The whole text handed to the model: instructions, then the input's
/// sections. Also what the observation's `input_hash` is taken over.
pub fn render(input: &ExtractInput) -> String {
    let mut text = String::with_capacity(INSTRUCTIONS.len() + 1024);
    text.push_str(INSTRUCTIONS);
    text.push_str("\n\n<plan>\n");
    text.push_str(input.plan.as_deref().unwrap_or("(no plan recorded)"));
    text.push_str("\n</plan>\n\n<entities>\n");
    text.push_str(if input.index.is_empty() {
        "(none yet)"
    } else {
        &input.index
    });
    text.push_str("\n</entities>\n\n<heads>\n");
    text.push_str(if input.heads.is_empty() {
        "(none yet)"
    } else {
        &input.heads
    });
    text.push_str("\n</heads>\n\n<turns>\n");
    text.push_str(&input.conversation());
    text.push_str("</turns>\n");
    text
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProposedEntity {
    pub slug: String,
    #[serde(default)]
    pub kind: EntityKind,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Quote {
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProposedAssertion {
    #[serde(default)]
    pub about: Vec<String>,
    pub kind: AssertionKind,
    pub domain: Domain,
    pub stance: Stance,
    pub statement: String,
    #[serde(default)]
    pub rationale: String,
    /// Whether the user said it is not the model's to claim: it follows from
    /// which turn its `evidence` is found in (see `pipeline`). An unknown
    /// field such as the old `stated_by_user` is ignored.
    #[serde(default)]
    pub relation: Option<ProposedRelation>,
    #[serde(default)]
    pub evidence: Vec<Quote>,
}

/// What the model proposed, after the items that did not fit were dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    pub entities: Vec<ProposedEntity>,
    pub assertions: Vec<ProposedAssertion>,
    /// Items that were not of the expected shape.
    pub malformed: usize,
}

/// Parse the model's answer. `Err` carries why when no JSON object could be
/// read at all; a malformed item inside a well-formed object is counted and
/// skipped instead.
pub fn parse(text: &str) -> std::result::Result<Parsed, String> {
    let body = strip_fences(text);
    let object = object_span(body).ok_or_else(|| "no JSON object in the output".to_string())?;
    let value: Value = serde_json::from_str(object).map_err(|e| e.to_string())?;
    let Value::Object(map) = value else {
        return Err("the output is not a JSON object".to_string());
    };
    let mut parsed = Parsed::default();
    for item in array(map.get("entities")) {
        match serde_json::from_value::<ProposedEntity>(item.clone()) {
            Ok(e) if !e.slug.trim().is_empty() => parsed.entities.push(e),
            Ok(_) => parsed.malformed += 1,
            Err(e) => {
                tracing::debug!(error = %e, "context extract: malformed entity skipped");
                parsed.malformed += 1;
            }
        }
    }
    for item in array(map.get("assertions")) {
        match serde_json::from_value::<ProposedAssertion>(item.clone()) {
            Ok(a) if !a.statement.trim().is_empty() => parsed.assertions.push(a),
            Ok(_) => parsed.malformed += 1,
            Err(e) => {
                tracing::debug!(error = %e, "context extract: malformed assertion skipped");
                parsed.malformed += 1;
            }
        }
    }
    Ok(parsed)
}

fn array(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// The text inside a ```-fence when the answer is wrapped in one (with or
/// without a language tag), else the text as it came.
fn strip_fences(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let rest = rest.split_once('\n').map_or("", |(_, body)| body);
    rest.rsplit_once("```")
        .map_or(rest, |(body, _)| body)
        .trim()
}

/// From the first `{` to the last `}`: prose on either side is dropped.
fn object_span(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}
