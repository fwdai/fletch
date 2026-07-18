//! Diff statistics and range diffs: shortstat/numstat/`diff <a>..<b>` and the
//! small text parsers that back them.

use std::path::Path;

use crate::error::Result;

use super::cmd::run_git;

/// Run `git diff --shortstat <a>..<b>` and parse the additions /
/// deletions counts. Returns zero counts if both refs resolve to the
/// same commit (git prints nothing in that case).
pub async fn diff_shortstat(repo: &Path, from_sha: &str, to_sha: &str) -> Result<(u32, u32)> {
    let range = format!("{from_sha}..{to_sha}");
    let out = run_git(repo, &["diff", "--shortstat", &range], "diff --shortstat").await?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(parse_shortstat(&line))
}

/// Run `git diff --shortstat <base>` from a live checkout. This compares the
/// current working tree, including uncommitted changes, against the base ref.
pub async fn checkout_diff_shortstat(checkout: &Path, base_ref: &str) -> Result<(u32, u32)> {
    let out = run_git(
        checkout,
        &["diff", "--shortstat", base_ref],
        &format!("diff --shortstat {base_ref}"),
    )
    .await?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(parse_shortstat(&line))
}

/// Per-file additions/deletions for `git diff --numstat <from>..<to>` in `repo`.
/// Binary files (numstat prints `-`/`-`) report zero counts. Lists the files a
/// ferried ref changed versus the run base for the review surface's file list.
pub async fn diff_numstat(
    repo: &Path,
    from_sha: &str,
    to_sha: &str,
) -> Result<Vec<(String, u32, u32)>> {
    let range = format!("{from_sha}..{to_sha}");
    let out = run_git(repo, &["diff", "--numstat", &range], "diff --numstat").await?;
    Ok(parse_numstat_lines(&String::from_utf8_lossy(&out.stdout)))
}

/// Parse `git diff --numstat` output into `(path, additions, deletions)` rows.
/// Binary files print `-` for both counts; those become 0.
fn parse_numstat_lines(text: &str) -> Vec<(String, u32, u32)> {
    let mut files = Vec::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        if let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) {
            files.push((
                path.to_string(),
                a.parse::<u32>().unwrap_or(0),
                d.parse::<u32>().unwrap_or(0),
            ));
        }
    }
    files
}

/// The unified diff of `from_sha..to_sha` in `repo`, optionally scoped to one
/// `path` (`-U3`, matching the file-diff view's context). Both refs are objects
/// in the same repo — for the review surface, the run repo where the ferried step
/// ref and the run base both live — so no checkout is needed.
pub async fn diff_refs(
    repo: &Path,
    from_sha: &str,
    to_sha: &str,
    path: Option<&str>,
) -> Result<String> {
    let range = format!("{from_sha}..{to_sha}");
    let mut args = vec!["diff", "--no-color", "-U3", &range];
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let out = run_git(repo, &args, "diff -U3 range").await?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn parse_shortstat(s: &str) -> (u32, u32) {
    let mut adds = 0u32;
    let mut dels = 0u32;
    for chunk in s.split(',').map(|c| c.trim()) {
        let mut parts = chunk.splitn(2, ' ');
        let n: u32 = match parts.next().and_then(|t| t.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let label = parts.next().unwrap_or("");
        if label.starts_with("insertion") {
            adds = n;
        } else if label.starts_with("deletion") {
            dels = n;
        }
    }
    (adds, dels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numstat_lines_counts_and_binaries() {
        let text = "12\t3\tsrc/a.rs\n-\t-\tassets/logo.png\n0\t5\tsrc/b.rs\n";
        assert_eq!(
            parse_numstat_lines(text),
            vec![
                ("src/a.rs".to_string(), 12, 3),
                ("assets/logo.png".to_string(), 0, 0),
                ("src/b.rs".to_string(), 0, 5),
            ]
        );
    }

    #[test]
    fn parse_shortstat_typical() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 82 insertions(+), 12 deletions(-)"),
            (82, 12)
        );
    }

    #[test]
    fn parse_shortstat_only_additions() {
        assert_eq!(parse_shortstat(" 1 file changed, 5 insertions(+)"), (5, 0));
    }

    #[test]
    fn parse_shortstat_only_deletions() {
        assert_eq!(parse_shortstat(" 2 files changed, 9 deletions(-)"), (0, 9));
    }

    #[test]
    fn parse_shortstat_empty() {
        assert_eq!(parse_shortstat(""), (0, 0));
    }
}
