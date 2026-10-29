//! The layers a box's egress allow-list is built from, most general first:
//!
//! 1. **The template** (`fletch_box::egress::TEMPLATE`), maintained in code.
//! 2. **The user's global list** ([`GLOBAL_SETTING`]): hosts added for every
//!    project, template entries turned off.
//! 3. **The project's settings files** (`crate::project_file`): `egress.allow`
//!    hosts the user approved, `egress.disable` inherited entries turned off.
//! 4. **The project's in-app additions** (`box.egress_allow`), from before the
//!    files existed.
//!
//! Each layer stores only its own changes ([`Delta`]), never a copy of what it
//! inherits, so a template entry added in a later release reaches everyone who
//! did not turn it off. A layer first turns off what it removes (by exact
//! pattern), then adds what it adds; a later layer can re-allow what an
//! earlier one turned off.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Global setting: the user's [`Delta`] over the template, as JSON.
pub const GLOBAL_SETTING: &str = "box_egress_global";

/// One layer's changes to what it inherits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delta {
    /// Patterns this layer allows.
    #[serde(default)]
    pub add: Vec<String>,
    /// Inherited patterns this layer turns off, matched exactly.
    #[serde(default)]
    pub remove: Vec<String>,
}

impl Delta {
    /// Every pattern in `HostPattern`'s spelling, each listed once; refused
    /// when one does not parse.
    pub fn normalized(&self) -> Result<Delta> {
        Ok(Delta {
            add: normalize_all(&self.add)?,
            remove: normalize_all(&self.remove)?,
        })
    }

    /// The delta a stored JSON value holds; an unreadable one is empty, so a
    /// hand-edited row never blocks a launch.
    pub fn parse(raw: Option<&str>) -> Delta {
        raw.and_then(|r| serde_json::from_str::<Delta>(r).ok())
            .and_then(|d| d.normalized().ok())
            .unwrap_or_default()
    }
}

fn normalize_all(patterns: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for pattern in patterns {
        let spelled = pattern
            .parse::<fletch_box::HostPattern>()
            .map_err(|e| Error::Other(e.to_string()))?
            .to_string();
        if !out.contains(&spelled) {
            out.push(spelled);
        }
    }
    Ok(out)
}

/// Where an effective entry comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Template,
    Global,
    ProjectFile,
    Project,
}

/// One pattern a box may reach, and which layer put it there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub pattern: String,
    pub source: Source,
    /// The template's category (`fletch_box::egress::Category::as_str`), for
    /// a template entry.
    pub category: Option<&'static str>,
}

/// The template, then each layer in order. See the module doc.
pub fn effective(layers: &[(Source, &Delta)]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = fletch_box::egress::TEMPLATE
        .iter()
        .map(|(pattern, category)| Entry {
            pattern: pattern.to_string(),
            source: Source::Template,
            category: Some(category.as_str()),
        })
        .collect();
    for (source, delta) in layers {
        entries.retain(|e| !delta.remove.contains(&e.pattern));
        for pattern in &delta.add {
            if !entries.iter().any(|e| &e.pattern == pattern) {
                entries.push(Entry {
                    pattern: pattern.clone(),
                    source: *source,
                    category: None,
                });
            }
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(add: &[&str], remove: &[&str]) -> Delta {
        Delta {
            add: add.iter().map(|s| s.to_string()).collect(),
            remove: remove.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn patterns(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.pattern.as_str()).collect()
    }

    /// With no changes the template is the list, each entry with its category.
    #[test]
    fn the_template_is_the_starting_point() {
        let entries = effective(&[]);
        assert_eq!(entries.len(), fletch_box::egress::TEMPLATE.len());
        assert!(entries
            .iter()
            .all(|e| e.source == Source::Template && e.category.is_some()));
    }

    /// The global list adds and turns off; a project turns off what the
    /// global list added and re-allows what it turned off — the most specific
    /// layer wins — and each entry says where it came from.
    #[test]
    fn later_layers_win() {
        let global = delta(&["internal.example.com", "a.example"], &["rubygems.org"]);
        let project = delta(&["rubygems.org"], &["a.example", "bitbucket.org"]);
        let entries = effective(&[(Source::Global, &global), (Source::ProjectFile, &project)]);
        let list = patterns(&entries);
        assert!(list.contains(&"internal.example.com"));
        assert!(!list.contains(&"a.example"));
        assert!(!list.contains(&"bitbucket.org"));
        let gems = entries
            .iter()
            .find(|e| e.pattern == "rubygems.org")
            .unwrap();
        assert_eq!(gems.source, Source::ProjectFile);
        let internal = entries
            .iter()
            .find(|e| e.pattern == "internal.example.com")
            .unwrap();
        assert_eq!(internal.source, Source::Global);
    }

    /// A stored delta is normalized; a bad pattern refuses a write but never
    /// breaks a read.
    #[test]
    fn deltas_normalize_and_bad_rows_read_empty() {
        let d = delta(&["Example.COM", "example.com"], &["*.X.org"]);
        assert_eq!(
            d.normalized().unwrap(),
            delta(&["example.com"], &["*.x.org"])
        );
        assert!(delta(&["not a host"], &[]).normalized().is_err());
        assert_eq!(Delta::parse(Some("{ nope")), Delta::default());
        assert_eq!(
            Delta::parse(Some(r#"{"add":["not a host"]}"#)),
            Delta::default()
        );
        assert_eq!(
            Delta::parse(Some(r#"{"add":["A.com"]}"#)),
            delta(&["a.com"], &[])
        );
    }
}
