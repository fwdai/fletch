//! Working-tree file listing, single-file show, and per-file unified diffs
//! (with the changed-line parser that drives the File panel gutter).

use std::path::Path;

use crate::error::{Error, Result};

use super::cmd::{git_output, run_git};

/// List the checkout's relevant files: everything tracked plus untracked
/// files that aren't gitignored. Paths are repo-relative with forward
/// slashes (git's native form). This is what the File panel browses — it
/// naturally excludes `node_modules`, build output, etc.
pub async fn list_files(checkout: &Path) -> Result<Vec<String>> {
    let out = run_git(
        checkout,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
        "ls-files",
    )
    .await?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect())
}

/// Read a single file's contents at a given ref (e.g. the parent branch),
/// used to show the prior contents of a file the agent deleted.
pub async fn show_file(checkout: &Path, base_ref: &str, path: &str) -> Result<String> {
    let spec = format!("{base_ref}:{path}");
    let out = run_git(
        checkout,
        &["show", &spec],
        &format!("show {base_ref}:{path}"),
    )
    .await?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Return the 1-indexed line numbers (in the current working-tree file) the
/// agent changed versus `base_ref`, split into purely-added lines and
/// modified lines. Drives the File panel's VS Code-style change gutter.
pub async fn file_changed_lines(
    checkout: &Path,
    base_ref: &str,
    path: &str,
) -> Result<(Vec<u32>, Vec<u32>)> {
    let diff = file_diff_unified(checkout, base_ref, path, "-U0").await?;
    Ok(parse_changed_lines(&diff))
}

/// Return the full unified diff of `path` versus `base_ref`, for the Code
/// panel's live view. `-U3` gives three lines of surrounding context per hunk.
pub async fn file_diff(checkout: &Path, base_ref: &str, path: &str) -> Result<String> {
    file_diff_unified(checkout, base_ref, path, "-U3").await
}

/// Unified diff of `path` versus `base_ref` with the given `-U<n>` context
/// flag. `git diff <ref>` only covers files in the index, so an untracked
/// (agent-created, never `git add`ed) file diffs as empty; when that happens,
/// re-diff it against /dev/null with `--no-index`, which renders the whole
/// file as one added hunk.
async fn file_diff_unified(
    checkout: &Path,
    base_ref: &str,
    path: &str,
    unified: &str,
) -> Result<String> {
    let out = run_git(
        checkout,
        &["diff", "--no-color", unified, base_ref, "--", path],
        &format!("diff {unified} {base_ref} -- {path}"),
    )
    .await?;
    if !out.stdout.is_empty() || is_tracked(checkout, path).await {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    // `--no-index` exits 1 whenever the two sides differ, so accept 0 and 1.
    let out = git_output(
        checkout,
        &[
            "diff",
            "--no-color",
            unified,
            "--no-index",
            "--",
            NULL_DEVICE,
            path,
        ],
    )
    .await?;
    match out.status.code() {
        Some(0) | Some(1) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        _ => Err(Error::Git(format!(
            "diff --no-index -- {path} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// The null device handed to `diff --no-index` as the "before" side. The app
/// ships macOS-only today, but name the Windows device explicitly rather than
/// relying on Git-for-Windows' msys `/dev/null` emulation if that changes.
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// Whether `path` is in the index. `ls-files --error-unmatch` exits 0 for
/// tracked paths; a spawn failure counts as tracked so callers fall back to
/// the plain-diff result rather than a second diff that would also fail.
async fn is_tracked(checkout: &Path, path: &str) -> bool {
    git_output(checkout, &["ls-files", "--error-unmatch", "--", path])
        .await
        .map(|o| o.status.success())
        .unwrap_or(true)
}

/// Parse `git diff -U0` output into (added, modified) new-file line numbers.
/// A hunk that only inserts lines marks them "added"; a hunk that also
/// removes lines marks its inserted lines "modified" (a replacement).
fn parse_changed_lines(diff: &str) -> (Vec<u32>, Vec<u32>) {
    let mut added: Vec<u32> = Vec::new();
    let mut modified: Vec<u32> = Vec::new();
    let mut new_line: u32 = 0;
    let mut hunk_added: Vec<u32> = Vec::new();
    let mut hunk_has_del = false;

    let flush = |hunk_added: &mut Vec<u32>,
                 hunk_has_del: &mut bool,
                 added: &mut Vec<u32>,
                 modified: &mut Vec<u32>| {
        if *hunk_has_del {
            modified.append(hunk_added);
        } else {
            added.append(hunk_added);
        }
        hunk_added.clear();
        *hunk_has_del = false;
    };

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            flush(
                &mut hunk_added,
                &mut hunk_has_del,
                &mut added,
                &mut modified,
            );
            // rest looks like " -a,b +c,d @@ ..."; take the "+c[,d]" token.
            if let Some(plus) = rest.split_whitespace().find(|t| t.starts_with('+')) {
                new_line = plus
                    .trim_start_matches('+')
                    .split(',')
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                    .unwrap_or(0);
            }
            continue;
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        match line.chars().next() {
            Some('+') => {
                hunk_added.push(new_line);
                new_line = new_line.saturating_add(1);
            }
            Some('-') => hunk_has_del = true,
            Some('\\') => {} // "\ No newline at end of file" — ignore
            _ => new_line = new_line.saturating_add(1), // context line
        }
    }
    flush(
        &mut hunk_added,
        &mut hunk_has_del,
        &mut added,
        &mut modified,
    );
    (added, modified)
}

#[cfg(test)]
mod changed_lines_tests {
    use super::parse_changed_lines;

    #[test]
    fn pure_additions_are_added() {
        let diff = "\
diff --git a/f b/f
--- a/f
+++ b/f
@@ -0,0 +1,3 @@
+one
+two
+three";
        assert_eq!(parse_changed_lines(diff), (vec![1, 2, 3], vec![]));
    }

    #[test]
    fn replacement_lines_are_modified() {
        // two old lines replaced by two new ones at line 3
        let diff = "\
@@ -3,2 +3,2 @@
-old a
-old b
+new a
+new b";
        assert_eq!(parse_changed_lines(diff), (vec![], vec![3, 4]));
    }

    #[test]
    fn mixed_hunks() {
        let diff = "\
@@ -3,1 +3,2 @@
-was
+now
+extra
@@ -10,0 +11,1 @@
+appended";
        // first hunk has a deletion → its '+' lines are modified (3,4);
        // second hunk is pure addition → added (11).
        assert_eq!(parse_changed_lines(diff), (vec![11], vec![3, 4]));
    }

    #[test]
    fn no_newline_marker_ignored() {
        let diff = "\
@@ -1 +1 @@
-a
+b
\\ No newline at end of file";
        assert_eq!(parse_changed_lines(diff), (vec![], vec![1]));
    }
}
