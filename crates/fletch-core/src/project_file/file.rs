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
    /// This file's `egress.allow`, as read.
    pub allow: Vec<String>,
    /// This file's `egress.disable`, as read.
    pub disable: Vec<String>,
}

/// What one file asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    /// Host patterns to allow, normalized (`HostPattern`'s spelling) and
    /// deduplicated.
    pub egress_allow: Vec<String>,
    /// Inherited patterns to turn off, likewise.
    pub egress_disable: Vec<String>,
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
        let list = match key.as_str() {
            "allow" => &mut parsed.egress_allow,
            "disable" => &mut parsed.egress_disable,
            _ => {
                parsed
                    .warnings
                    .push(format!("unknown setting `egress.{key}`"));
                continue;
            }
        };
        let Value::Array(items) = value else {
            parsed
                .warnings
                .push(format!("`egress.{key}` must be a list of host patterns"));
            continue;
        };
        for item in items {
            let Value::String(raw) = &item else {
                parsed
                    .warnings
                    .push(format!("`egress.{key}`: {item} is not a string"));
                continue;
            };
            match raw.parse::<fletch_box::HostPattern>() {
                Ok(pattern) => {
                    let spelled = pattern.to_string();
                    if !list.contains(&spelled) {
                        list.push(spelled);
                    }
                }
                Err(e) => parsed.warnings.push(format!("`egress.{key}`: {e}")),
            }
        }
    }
}

/// Whether `text` holds a `//` or `/* */` comment outside a string: what an
/// edit from the app, which writes plain JSON, would lose.
pub fn has_comments(text: &str) -> bool {
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => in_string = false,
                _ => {}
            }
        } else if c == '"' {
            in_string = true;
        } else if c == '/' && matches!(chars.peek(), Some('/') | Some('*')) {
            return true;
        }
    }
    false
}

/// Which of the two files an edit goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Which {
    /// `.fletch/settings.json`, the team's.
    Shared,
    /// `.fletch/settings.local.json`, the user's own.
    Local,
}

impl Which {
    pub fn file_name(self) -> &'static str {
        match self {
            Which::Shared => SHARED_FILE,
            Which::Local => LOCAL_FILE,
        }
    }
}

/// An edit to one file's `egress` lists: each pattern is normalized; adds go
/// after removes.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    #[serde(default)]
    pub allow_add: Vec<String>,
    #[serde(default)]
    pub allow_remove: Vec<String>,
    #[serde(default)]
    pub disable_add: Vec<String>,
    #[serde(default)]
    pub disable_remove: Vec<String>,
}

/// `text` (a settings file, or `None` for one that does not exist yet) with
/// `edit` applied, as the JSON to write. Refused for a file with comments,
/// which plain JSON would drop — that file is edited by hand — and for a
/// pattern that does not parse. Everything else in the file is kept; an
/// emptied list, and then an emptied `egress`, is left out.
pub fn edit_text(text: Option<&str>, edit: &Edit) -> Result<String, String> {
    let text = text.unwrap_or("{}");
    if has_comments(text) {
        return Err(
            "this file has comments, which an edit from Fletch would drop; edit it by \
                    hand instead"
                .into(),
        );
    }
    let value: Value =
        serde_json::from_str(&strip_jsonc(text)).map_err(|e| format!("not valid JSON: {e}"))?;
    let Value::Object(mut top) = value else {
        return Err("the file must hold a JSON object".into());
    };
    let mut egress = match top.remove("egress") {
        Some(Value::Object(egress)) => egress,
        Some(_) => return Err("`egress` must be an object".into()),
        None => serde_json::Map::new(),
    };
    for (key, remove, add) in [
        ("allow", &edit.allow_remove, &edit.allow_add),
        ("disable", &edit.disable_remove, &edit.disable_add),
    ] {
        let mut list: Vec<String> = match egress.remove(key) {
            Some(Value::Array(items)) => items
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            Some(_) => return Err(format!("`egress.{key}` must be a list")),
            None => Vec::new(),
        };
        let normalize = |raw: &String| {
            raw.parse::<fletch_box::HostPattern>()
                .map(|p| p.to_string())
                .map_err(|e| e.to_string())
        };
        for raw in remove {
            let spelled = normalize(raw)?;
            list.retain(|p| {
                p.parse::<fletch_box::HostPattern>()
                    .map(|q| q.to_string() != spelled)
                    .unwrap_or(true)
            });
        }
        for raw in add {
            let spelled = normalize(raw)?;
            if !list.contains(&spelled) {
                list.push(spelled);
            }
        }
        if !list.is_empty() {
            egress.insert(
                key.to_string(),
                Value::Array(list.into_iter().map(Value::String).collect()),
            );
        }
    }
    if !egress.is_empty() {
        top.insert("egress".into(), Value::Object(egress));
    }
    let mut out = serde_json::to_string_pretty(&Value::Object(top)).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(out)
}

/// The repository's `info/exclude`, the git-ignore that is never committed:
/// `.git/info/exclude`, or for a worktree (`.git` a file naming its git dir)
/// the common dir's. `None` when `repo` is not a git checkout.
pub fn exclude_file(repo: &Path) -> Option<PathBuf> {
    let dot_git = repo.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let dir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        let dir = if dir.is_absolute() {
            dir
        } else {
            repo.join(dir)
        };
        match std::fs::read_to_string(dir.join("commondir")) {
            Ok(common) => {
                let common = PathBuf::from(common.trim());
                if common.is_absolute() {
                    common
                } else {
                    dir.join(common)
                }
            }
            Err(_) => dir,
        }
    };
    Some(git_dir.join("info").join("exclude"))
}

/// Keep `.fletch/settings.local.json` out of git, through `info/exclude` so
/// the user's tracked `.gitignore` is never touched. Idempotent; a repository
/// that is not a git checkout has nothing to exclude from.
pub fn exclude_local_file(repo: &Path) -> std::io::Result<()> {
    let Some(exclude) = exclude_file(repo) else {
        return Ok(());
    };
    let line = format!("/{DIR}/{LOCAL_FILE}");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    if let Some(dir) = exclude.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let sep = if current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::write(&exclude, format!("{current}{sep}{line}\n"))
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
    /// Both files' turned-off patterns, likewise.
    pub egress_disable: Vec<String>,
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
                            for (from, into) in [
                                (&parsed.egress_allow, &mut read.egress_allow),
                                (&parsed.egress_disable, &mut read.egress_disable),
                            ] {
                                for pattern in from {
                                    if !into.contains(pattern) {
                                        into.push(pattern.clone());
                                    }
                                }
                            }
                            report.allow = parsed.egress_allow;
                            report.disable = parsed.egress_disable;
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

    /// `egress.disable` is read like `allow`, and each file reports its own
    /// lists.
    #[test]
    fn reads_disable_and_reports_each_files_lists() {
        let parsed =
            parse(r#"{ "egress": { "allow": ["a.com"], "disable": ["RubyGems.org", 7] } }"#)
                .unwrap();
        assert_eq!(parsed.egress_disable, vec!["rubygems.org"]);
        assert_eq!(parsed.warnings.len(), 1);

        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        write(
            &repo,
            SHARED_FILE,
            r#"{ "egress": { "disable": ["a.org"] } }"#,
        );
        write(&repo, LOCAL_FILE, r#"{ "egress": { "allow": ["b.org"] } }"#);
        let read = read_files(&repo, None);
        assert_eq!(read.egress_disable, vec!["a.org"]);
        assert_eq!(read.files[0].disable, vec!["a.org"]);
        assert_eq!(read.files[1].allow, vec!["b.org"]);
    }

    /// Comments are found outside strings only.
    #[test]
    fn finds_comments_outside_strings() {
        assert!(has_comments("{ // x\n }"));
        assert!(has_comments("{ /* x */ }"));
        assert!(!has_comments(r#"{ "url": "https://x//y/*z*/" }"#));
        assert!(!has_comments(r#"{ "q": "a\"//" }"#));
    }

    /// An edit adds and removes in both lists, keeps the rest of the file,
    /// drops what it emptied, and refuses a file with comments or a bad
    /// pattern.
    #[test]
    fn edits_the_egress_lists() {
        let out = edit_text(
            None,
            &Edit {
                allow_add: vec!["A.com".into(), "b.com".into()],
                disable_add: vec!["rubygems.org".into()],
                ..Edit::default()
            },
        )
        .unwrap();
        let parsed = parse(&out).unwrap();
        assert_eq!(parsed.egress_allow, vec!["a.com", "b.com"]);
        assert_eq!(parsed.egress_disable, vec!["rubygems.org"]);

        let out = edit_text(
            Some(&format!(
                r#"{{ "$schema": "s", {} }}"#,
                &out[1..out.len() - 2]
            )),
            &Edit {
                allow_remove: vec!["a.com".into()],
                disable_remove: vec!["rubygems.org".into()],
                ..Edit::default()
            },
        )
        .unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["$schema"], "s");
        assert_eq!(value["egress"]["allow"], serde_json::json!(["b.com"]));
        assert!(value["egress"].get("disable").is_none());

        let emptied = edit_text(
            Some(r#"{ "egress": { "allow": ["b.com"] } }"#),
            &Edit {
                allow_remove: vec!["b.com".into()],
                ..Edit::default()
            },
        )
        .unwrap();
        assert_eq!(emptied.trim(), "{}");

        assert!(edit_text(Some("{ // mine\n }"), &Edit::default()).is_err());
        let bad = Edit {
            allow_add: vec!["not a host".into()],
            ..Edit::default()
        };
        assert!(edit_text(None, &bad).is_err());
    }

    /// The local file is excluded through the repository's `info/exclude`,
    /// once; a worktree's goes to the common dir; a non-repository is left
    /// alone.
    #[test]
    fn excludes_the_local_file_from_git() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        exclude_local_file(&repo).unwrap();
        exclude_local_file(&repo).unwrap();
        let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude, "/.fletch/settings.local.json\n");

        let common = td.path().join("main.git");
        let worktree_git = common.join("worktrees").join("wt");
        std::fs::create_dir_all(&worktree_git).unwrap();
        std::fs::write(worktree_git.join("commondir"), "../..").unwrap();
        let wt = td.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )
        .unwrap();
        assert_eq!(
            exclude_file(&wt).unwrap(),
            worktree_git.join("../..").join("info").join("exclude")
        );

        assert!(exclude_file(&td.path().join("plain")).is_none());
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
