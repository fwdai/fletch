//! Reading the files themselves: where they are, what they say, and the
//! refusal to read an agent's checkout. Depends on nothing of the engine's, so
//! it can be reasoned about (and tested) on its own.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use serde_json::Value;

/// The folder, at the root of the project's primary repo.
pub const DIR: &str = ".fletch";
/// The team's file, meant to be committed.
pub const SHARED_FILE: &str = "settings.json";
/// One person's file, beside it.
pub const LOCAL_FILE: &str = "settings.local.json";

/// A settings file is a few lines; anything this big is not one.
const MAX_FILE_BYTES: u64 = 256 * 1024;

/// One file, as last read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FileReport {
    /// Repo-relative: `.fletch/settings.json`.
    pub path: String,
    pub exists: bool,
    /// Why nothing in it applies: unreadable, too large, not JSON.
    pub error: Option<String>,
    /// What was skipped while the rest applied: an unknown key, a value that
    /// is not a host pattern.
    pub warnings: Vec<String>,
}

/// What one file asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    /// Normalized host patterns (`HostPattern`'s spelling), deduplicated.
    pub egress_allow: Vec<String>,
    pub warnings: Vec<String>,
}

/// Parse a settings file: JSON with `//` and `/* */` comments and trailing
/// commas. An unknown key or a bad entry is a warning and is skipped; text
/// that is not a JSON object is an error.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let json = strip_jsonc(text);
    let value: Value = serde_json::from_str(&json).map_err(|e| format!("not valid JSON: {e}"))?;
    let Value::Object(top) = value else {
        return Err("the file must hold a JSON object".into());
    };
    let mut parsed = Parsed::default();
    for (key, value) in top {
        match key.as_str() {
            "$schema" => {}
            "egress" => parse_egress(value, &mut parsed),
            _ => parsed.warnings.push(format!("unknown setting `{key}`")),
        }
    }
    Ok(parsed)
}

fn parse_egress(value: Value, parsed: &mut Parsed) {
    let Value::Object(egress) = value else {
        parsed.warnings.push("`egress` must be an object".into());
        return;
    };
    for (key, value) in egress {
        if key != "allow" {
            parsed
                .warnings
                .push(format!("unknown setting `egress.{key}`"));
            continue;
        }
        let Value::Array(items) = value else {
            parsed
                .warnings
                .push("`egress.allow` must be a list of host patterns".into());
            continue;
        };
        for item in items {
            let Value::String(raw) = &item else {
                parsed
                    .warnings
                    .push(format!("`egress.allow`: {item} is not a string"));
                continue;
            };
            match raw.parse::<fletch_box::HostPattern>() {
                Ok(pattern) => {
                    let spelled = pattern.to_string();
                    if !parsed.egress_allow.contains(&spelled) {
                        parsed.egress_allow.push(spelled);
                    }
                }
                Err(e) => parsed.warnings.push(format!("`egress.allow`: {e}")),
            }
        }
    }
}

/// JSON with comments made JSON: `//` line and `/* */` block comments become
/// spaces (newlines kept, so serde's line numbers still point into the file),
/// and a comma whose next significant character closes an object or array is
/// dropped. Text inside strings is left alone.
fn strip_jsonc(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(&next) = chars.get(i + 1) {
                    out.push(next);
                    i += 1;
                }
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match (c, chars.get(i + 1)) {
            ('"', _) => {
                in_string = true;
                out.push(c);
                i += 1;
            }
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    out.push(' ');
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                out.push_str("  ");
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
                if i < chars.len() {
                    out.push_str("  ");
                    i += 2;
                }
            }
            (',', _) if closes_next(&chars, i + 1) => {
                out.push(' ');
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Whether the next significant character from `from` (skipping whitespace and
/// comments) is `}` or `]`.
fn closes_next(chars: &[char], mut from: usize) -> bool {
    while from < chars.len() {
        match (chars[from], chars.get(from + 1)) {
            (c, _) if c.is_whitespace() => from += 1,
            ('/', Some('/')) => {
                while from < chars.len() && chars[from] != '\n' {
                    from += 1;
                }
            }
            ('/', Some('*')) => {
                from += 2;
                while from < chars.len()
                    && !(chars[from] == '*' && chars.get(from + 1) == Some(&'/'))
                {
                    from += 1;
                }
                from += 2;
            }
            (c, _) => return c == '}' || c == ']',
        }
    }
    false
}

/// The files' size and mtime, for the watcher to tell a change.
pub type Stamp = Vec<Option<(u64, SystemTime)>>;

/// What [`read_files`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Read {
    /// Why the repository was not read at all.
    pub error: Option<String>,
    pub files: Vec<FileReport>,
    /// Both files' egress additions, the team's first, deduplicated.
    pub egress_allow: Vec<String>,
}

fn file_paths(repo: &Path) -> [(String, PathBuf); 2] {
    [SHARED_FILE, LOCAL_FILE].map(|name| (format!("{DIR}/{name}"), repo.join(DIR).join(name)))
}

pub fn stamp(repo: &Path) -> Stamp {
    file_paths(repo)
        .iter()
        .map(|(_, path)| {
            let meta = std::fs::metadata(path).ok()?;
            Some((meta.len(), meta.modified().ok()?))
        })
        .collect()
}

/// Read both files of the repository at `repo`. `agents_root` is where agents'
/// checkouts live: a repository under it is never read, since the agent can
/// write it (see the parent module).
pub fn read_files(repo: &Path, agents_root: Option<&Path>) -> Read {
    if let Some(root) = agents_root {
        let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if canonical(repo).starts_with(canonical(root)) {
            return Read {
                error: Some(format!(
                    "{} is inside an agent's workspace; project settings are read only from \
                     your own repository",
                    repo.display()
                )),
                ..Read::default()
            };
        }
    }
    let mut read = Read::default();
    for (shown, path) in file_paths(repo) {
        let mut report = FileReport {
            path: shown,
            ..FileReport::default()
        };
        match std::fs::metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                report.exists = true;
                report.error = Some(format!("unreadable: {e}"));
            }
            Ok(meta) if !meta.is_file() => {
                report.exists = true;
                report.error = Some("not a file".into());
            }
            Ok(meta) if meta.len() > MAX_FILE_BYTES => {
                report.exists = true;
                report.error = Some(format!(
                    "{} bytes is too large for a settings file",
                    meta.len()
                ));
            }
            Ok(_) => {
                report.exists = true;
                match std::fs::read_to_string(&path) {
                    Err(e) => report.error = Some(format!("unreadable: {e}")),
                    Ok(text) => match parse(&text) {
                        Err(e) => report.error = Some(e),
                        Ok(parsed) => {
                            report.warnings = parsed.warnings;
                            for pattern in parsed.egress_allow {
                                if !read.egress_allow.contains(&pattern) {
                                    read.egress_allow.push(pattern);
                                }
                            }
                        }
                    },
                }
            }
        }
        read.files.push(report);
    }
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(repo: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(repo.join(DIR)).unwrap();
        std::fs::write(repo.join(DIR).join(name), text).unwrap();
    }

    /// Comments and trailing commas are JSON with comments, not errors; a
    /// comment marker inside a string is text.
    #[test]
    fn parses_json_with_comments() {
        let parsed = parse(
            r#"{
              // the team's hosts
              "egress": {
                /* toolchains */
                "allow": [
                  "static.rust-lang.org",
                  "*.Internal.Example.com", // trailing comma next
                  "https://not-a-host//x",
                ],
              },
            }"#,
        )
        .unwrap();
        assert_eq!(
            parsed.egress_allow,
            vec!["static.rust-lang.org", "*.internal.example.com"]
        );
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        assert!(parsed.warnings[0].contains("not-a-host"));
    }

    /// Unknown keys and wrong shapes are skipped with a warning; the rest
    /// applies. Only text that is not a JSON object is an error.
    #[test]
    fn skips_what_it_does_not_know() {
        let parsed = parse(
            r#"{ "$schema": "x", "run": {}, "egress": { "deny": [], "allow": ["a.com", 3] } }"#,
        )
        .unwrap();
        assert_eq!(parsed.egress_allow, vec!["a.com"]);
        assert_eq!(parsed.warnings.len(), 3, "{:?}", parsed.warnings);
        assert!(parse("[1, 2]").is_err());
        assert!(parse("{ not json").is_err());
        assert_eq!(parse("{}").unwrap(), Parsed::default());
    }

    /// Both files are read, the team's first, their additions merged; a file
    /// that does not parse is reported and skipped, never failing the other.
    #[test]
    fn reads_both_files_and_reports_a_broken_one() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        write(
            &repo,
            SHARED_FILE,
            r#"{ "egress": { "allow": ["a.com", "b.com"] } }"#,
        );
        write(
            &repo,
            LOCAL_FILE,
            r#"{ "egress": { "allow": ["b.com", "c.com"] } }"#,
        );
        let read = read_files(&repo, None);
        assert_eq!(read.egress_allow, vec!["a.com", "b.com", "c.com"]);
        assert!(read.files.iter().all(|f| f.exists && f.error.is_none()));

        write(&repo, LOCAL_FILE, "{ oops");
        let read = read_files(&repo, None);
        assert_eq!(read.egress_allow, vec!["a.com", "b.com"]);
        assert!(read.files[1].error.is_some());

        let empty = read_files(&td.path().join("elsewhere"), None);
        assert!(empty.files.iter().all(|f| !f.exists));
        assert!(empty.egress_allow.is_empty());
    }

    /// A repository under the agents' workspaces root — an agent's checkout,
    /// which the agent can write — is never read, whatever its files say.
    #[test]
    fn never_reads_an_agents_checkout() {
        let td = tempfile::tempdir().unwrap();
        let agents = td.path().join("workspaces");
        let checkout = agents.join("lofoten").join("fletch");
        write(
            &checkout,
            SHARED_FILE,
            r#"{ "egress": { "allow": ["evil.example"] } }"#,
        );
        let read = read_files(&checkout, Some(&agents));
        assert!(read.error.is_some());
        assert!(read.egress_allow.is_empty());
        assert!(read.files.is_empty());

        let mine = td.path().join("code").join("fletch");
        write(
            &mine,
            SHARED_FILE,
            r#"{ "egress": { "allow": ["ok.example"] } }"#,
        );
        assert_eq!(
            read_files(&mine, Some(&agents)).egress_allow,
            vec!["ok.example"]
        );
    }

    /// The stamp moves when a file is written, and stays put otherwise.
    #[test]
    fn the_stamp_tells_a_change() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        assert_eq!(stamp(&repo), vec![None, None]);
        write(&repo, SHARED_FILE, "{}");
        let first = stamp(&repo);
        assert!(first[0].is_some());
        assert_eq!(stamp(&repo), first);
        write(&repo, SHARED_FILE, r#"{ "egress": {} }"#);
        assert_ne!(stamp(&repo), first);
    }
}
