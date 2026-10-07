use crate::capture::extract::prompt::parse;
use crate::context::model::*;

const GOOD: &str = r#"{
  "entities": [{"slug": "auth", "kind": "feature", "name": "Auth", "summary": "Sign-in"}],
  "assertions": [{
    "about": ["auth"], "kind": "decision", "domain": "architectural", "stance": "adopted",
    "statement": "Use JWT", "rationale": "stateless", "stated_by_user": true,
    "relation": {"kind": "new"}, "evidence": [{"quote": "Let's use JWT"}]
  }]
}"#;

#[test]
fn strict_json_parses_whole() {
    let parsed = parse(GOOD).unwrap();
    assert_eq!(parsed.entities.len(), 1);
    assert_eq!(parsed.entities[0].kind, EntityKind::Feature);
    assert_eq!(parsed.assertions.len(), 1);
    let a = &parsed.assertions[0];
    assert_eq!(a.kind, AssertionKind::Decision);
    // `stated_by_user` is no longer in the schema; an unknown field is ignored.
    assert_eq!(a.relation.as_ref().map(|r| r.kind), Some(RelationKind::New));
    assert_eq!(a.evidence[0].quote, "Let's use JWT");
    assert_eq!(parsed.malformed, 0);
}

#[test]
fn a_fenced_answer_with_prose_around_it_is_read() {
    let fenced = format!("Here you go:\n```json\n{GOOD}\n```\nLet me know.");
    assert_eq!(parse(&fenced).unwrap(), parse(GOOD).unwrap());
    let bare_fence = format!("```\n{GOOD}\n```");
    assert_eq!(parse(&bare_fence).unwrap(), parse(GOOD).unwrap());
}

#[test]
fn malformed_output_is_an_error() {
    assert!(parse("I could not find anything.").is_err());
    assert!(parse("{\"entities\": [").is_err());
    assert!(parse("[1, 2]").is_err());
}

#[test]
fn a_bad_item_is_skipped_and_counted_not_fatal() {
    let text = r#"{
      "entities": [{"kind": "feature"}, {"slug": "ok"}],
      "assertions": [{"about": ["ok"], "kind": "wish", "domain": "business", "stance": "adopted", "statement": "x"}]
    }"#;
    let parsed = parse(text).unwrap();
    assert_eq!(parsed.entities.len(), 1);
    assert_eq!(parsed.entities[0].slug, "ok");
    assert_eq!(parsed.entities[0].kind, EntityKind::Topic);
    assert!(parsed.assertions.is_empty());
    assert_eq!(parsed.malformed, 2);
}

#[test]
fn missing_arrays_mean_nothing_proposed() {
    assert_eq!(parse("{}").unwrap(), Default::default());
}
