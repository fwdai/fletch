//! Rendering a [`Bundle`] as the markdown an agent reads. Order: Vision ·
//! Entities and their relations · Decisions / constraints / facts grouped by
//! domain (adopted, then rejected) · History (if requested) · Warnings.
//! Provisional and contradicted assertions are marked inline, never dropped.
//!
//! Also the compact *index* the instruction block carries at spawn: every
//! active entity as `slug ("name")`, grouped by kind, capped. The index goes
//! into every agent's instructions, so names travel as quoted data and an
//! entity the extractor minted on its own is left out until a person, an
//! agent or a merged PR has revised it: model output never writes itself
//! into the next model's instructions.

use super::model::*;

const ENTITY_KINDS: [EntityKind; 6] = [
    EntityKind::Vision,
    EntityKind::Goal,
    EntityKind::Capability,
    EntityKind::Feature,
    EntityKind::Module,
    EntityKind::Topic,
];
const ASSERTION_KINDS: [AssertionKind; 3] = [
    AssertionKind::Decision,
    AssertionKind::Constraint,
    AssertionKind::Fact,
];
const DOMAINS: [Domain; 3] = [
    Domain::Business,
    Domain::Architectural,
    Domain::Implementation,
];
const STANCES: [Stance; 2] = [Stance::Adopted, Stance::Rejected];

pub fn render_markdown(bundle: &Bundle) -> String {
    let mut out = String::from("# Project context\n");
    out.push_str("\n## Vision\n");
    match (&bundle.vision, &bundle.vision_fallback) {
        (Some(v), _) => out.push_str(&v.summary),
        (None, Some(fallback)) => out.push_str(fallback),
        (None, None) => out.push_str("No vision recorded yet."),
    }
    out.push('\n');

    let listed: Vec<&BundleEntity> = bundle
        .entities
        .iter()
        .filter(|b| b.reason != EntryReason::Vision)
        .collect();
    if !listed.is_empty() {
        out.push_str("\n## Entities\n");
        for kind in ENTITY_KINDS {
            let group: Vec<&&BundleEntity> =
                listed.iter().filter(|b| b.entity.kind == kind).collect();
            if group.is_empty() {
                continue;
            }
            out.push_str(&format!("\n### {}\n", entity_kind_label(kind)));
            for b in group {
                let e = &b.entity;
                out.push_str(&format!(
                    "- **{}** (`{}`) — {}\n",
                    e.name, e.slug, e.summary
                ));
                for r in b.relations.iter().filter(|r| r.from == e.id) {
                    if let Some(to) = slug_of(bundle, &r.to) {
                        out.push_str(&format!("  - {} `{}`\n", rel_name(r.rel), to));
                    }
                }
            }
        }
    }

    for kind in ASSERTION_KINDS {
        let of_kind: Vec<&BundleAssertion> = bundle
            .assertions
            .iter()
            .filter(|a| a.assertion.kind == kind)
            .collect();
        if of_kind.is_empty() {
            continue;
        }
        out.push_str(&format!("\n## {}\n", assertion_kind_label(kind)));
        for domain in DOMAINS {
            let of_domain: Vec<&&BundleAssertion> = of_kind
                .iter()
                .filter(|a| a.assertion.domain == domain)
                .collect();
            if of_domain.is_empty() {
                continue;
            }
            out.push_str(&format!("\n### {}\n", domain_label(domain)));
            for stance in STANCES {
                for b in of_domain.iter().filter(|a| a.assertion.stance == stance) {
                    out.push_str(&assertion_line(bundle, b));
                }
            }
        }
    }

    if !bundle.warnings.is_empty() {
        out.push_str("\n## Warnings\n");
        for w in &bundle.warnings {
            out.push_str(&format!("- {w}\n"));
        }
    }
    out
}

fn slug_of<'a>(bundle: &'a Bundle, id: &str) -> Option<&'a str> {
    bundle
        .entities
        .iter()
        .map(|b| &b.entity)
        .chain(bundle.vision.iter())
        .find(|e| e.id == id)
        .map(|e| e.slug.as_str())
}

fn assertion_line(bundle: &Bundle, b: &BundleAssertion) -> String {
    let a = &b.assertion;
    let mut line = format!("- [{}] {}", stance_label(a.stance), a.statement);
    let about: Vec<String> = a
        .about
        .iter()
        .filter_map(|id| slug_of(bundle, id))
        .map(|s| format!("`{s}`"))
        .collect();
    if !about.is_empty() {
        line.push_str(&format!(" (about: {})", about.join(", ")));
    }
    if !a.rationale.is_empty() {
        line.push_str(&format!(" — {}", a.rationale));
    }
    if b.flags.provisional {
        line.push_str(" _(provisional)_");
    }
    for other in &b.flags.contradicted_by {
        line.push_str(&format!(" _(contradicted by {})_", short(other)));
    }
    line.push_str(&format!(" (id: {})\n", short(&a.id)));

    let mut newer = a;
    for prev in &b.history {
        let reasoning = newer
            .supersedes
            .as_ref()
            .map(|s| s.reasoning.as_str())
            .unwrap_or("");
        line.push_str(&format!(
            "  - superseded: {} — reasoning: {}\n",
            prev.statement, reasoning
        ));
        newer = prev;
    }
    line
}

/// The spawn-time index: slugs and names by kind, within `max_chars`. Empty
/// string when the graph has no active entities.
pub fn render_index(graph: &Graph, max_chars: usize) -> String {
    let groups: Vec<(&str, Vec<String>)> = ENTITY_KINDS
        .iter()
        .map(|kind| {
            let items = graph
                .entities
                .iter()
                .filter(|e| {
                    e.status == EntityStatus::Active
                        && e.kind == *kind
                        && e.author.kind != AuthorKind::Extractor
                })
                .map(|e| format!("{} ({})", e.slug, quoted(&e.name)))
                .collect::<Vec<_>>();
            (entity_kind_label(*kind), items)
        })
        .filter(|(_, items)| !items.is_empty())
        .collect();
    let total: usize = groups.iter().map(|(_, items)| items.len()).sum();
    if total == 0 {
        return String::new();
    }

    let full = index_text(&groups, usize::MAX, 0);
    if full.len() <= max_chars {
        return full;
    }
    let reserve = format!("… and {total} more").len();
    let (mut out, emitted) = index_text_counted(&groups, max_chars, reserve);
    out.push_str(&format!("… and {} more", total - emitted));
    out
}

fn index_text(groups: &[(&str, Vec<String>)], max_chars: usize, reserve: usize) -> String {
    index_text_counted(groups, max_chars, reserve).0
}

/// Writes group lines item by item while they fit under `max_chars - reserve`;
/// returns the text and how many items made it in.
fn index_text_counted(
    groups: &[(&str, Vec<String>)],
    max_chars: usize,
    reserve: usize,
) -> (String, usize) {
    let mut out = String::from("## Project context index\n");
    let mut emitted = 0;
    for (label, items) in groups {
        let mut line = format!("**{label}:** ");
        let mut any = false;
        for item in items {
            let sep = if any { ", " } else { "" };
            if out.len() + line.len() + sep.len() + item.len() + 1 + reserve > max_chars {
                if any {
                    out.push_str(&line);
                    out.push('\n');
                }
                return (out, emitted);
            }
            line.push_str(sep);
            line.push_str(item);
            any = true;
            emitted += 1;
        }
        out.push_str(&line);
        out.push('\n');
    }
    (out, emitted)
}

/// A name as a quoted string: whatever it contains reads as data.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn entity_kind_label(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Vision => "Visions",
        EntityKind::Goal => "Goals",
        EntityKind::Capability => "Capabilities",
        EntityKind::Feature => "Features",
        EntityKind::Module => "Modules",
        EntityKind::Topic => "Topics",
    }
}

fn assertion_kind_label(kind: AssertionKind) -> &'static str {
    match kind {
        AssertionKind::Decision => "Decisions",
        AssertionKind::Constraint => "Constraints",
        AssertionKind::Fact => "Facts",
    }
}

fn domain_label(domain: Domain) -> &'static str {
    match domain {
        Domain::Business => "Business",
        Domain::Architectural => "Architectural",
        Domain::Implementation => "Implementation",
    }
}

fn stance_label(stance: Stance) -> &'static str {
    match stance {
        Stance::Adopted => "adopted",
        Stance::Rejected => "rejected",
    }
}

fn rel_name(rel: Rel) -> &'static str {
    match rel {
        Rel::PartOf => "part_of",
        Rel::DependsOn => "depends_on",
        Rel::Serves => "serves",
    }
}

#[cfg(test)]
#[path = "tests/render.rs"]
mod tests;
