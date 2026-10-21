//! What a failing check left behind: the job log and the annotations. Read by
//! the agent's `pr_check_log` op once `pr_checks` has named the check; the id
//! on a [`CheckRun`] is the handle.
//!
//! A GitHub Actions job and its check run share one id, so the Actions log
//! endpoint takes the check-run id directly. Checks from other apps have no
//! Actions log (the endpoint answers 404) but may carry annotations, which
//! come from the check-runs API and work for every app.

use std::path::Path;

use serde_json::Value;

use crate::error::{Error, Result};

use super::client;
use super::query::require_repo_ref;
use super::types::*;

/// Most lines a caller may ask for; the whole log of a long job is megabytes.
pub const CHECK_LOG_MAX_LINES: usize = 2000;
pub const CHECK_LOG_DEFAULT_LINES: usize = 200;

/// The tail of check `id`'s job log and its annotations. Errors only when
/// nothing can be read: no token, no GitHub origin, a transport failure, or a
/// check with neither a log nor annotations GitHub will show.
pub async fn check_log(checkout: &Path, id: u64, lines: usize) -> Result<CheckLog> {
    let (owner, repo) = require_repo_ref(checkout).await?;
    let client = client::Client::new()?;

    let (log_status, raw) = client
        .rest_text(&format!("/repos/{owner}/{repo}/actions/jobs/{id}/logs"))
        .await?;
    let (log_tail, truncated, note) = if log_status.is_success() {
        let (tail, truncated) = tail_lines(&raw, lines);
        (Some(tail), truncated, None)
    } else {
        let why = match log_status.as_u16() {
            404 | 410 => "no job log for this check: it is not a GitHub Actions job, or its log \
                          has expired; read the annotations and the check's url"
                .to_string(),
            other => format!("the job log could not be read (GitHub answered {other})"),
        };
        (None, false, Some(why))
    };

    let (ann_status, body) = client
        .rest(
            reqwest::Method::GET,
            &format!("/repos/{owner}/{repo}/check-runs/{id}/annotations?per_page=100"),
            None,
        )
        .await?;
    let annotations = if ann_status.is_success() {
        parse_annotations(&body)
    } else if log_tail.is_none() {
        return Err(Error::Gh(format!(
            "check {id} was not found in this repository, or is not visible to the host's \
             GitHub account"
        )));
    } else {
        Vec::new()
    };

    Ok(CheckLog {
        id,
        log_tail,
        truncated,
        annotations,
        note,
    })
}

/// The last `lines` lines of an Actions log, each stripped of the leading
/// `2026-10-21T10:00:00.1234567Z ` stamp the runner adds — the agents that
/// read logs through the CLI piped them through `sed` for exactly that.
/// Returns the tail and whether anything was cut.
pub(crate) fn tail_lines(log: &str, lines: usize) -> (String, bool) {
    let lines = lines.clamp(1, CHECK_LOG_MAX_LINES);
    let all: Vec<&str> = log.lines().collect();
    let skip = all.len().saturating_sub(lines);
    let tail = all[skip..]
        .iter()
        .map(|l| strip_runner_timestamp(l))
        .collect::<Vec<_>>()
        .join("\n");
    (tail, skip > 0)
}

/// `2026-10-21T10:00:00.1234567Z rest` → `rest`; a line without the stamp is
/// returned whole.
fn strip_runner_timestamp(line: &str) -> &str {
    let Some((stamp, rest)) = line.split_once(' ') else {
        return line;
    };
    let looks_like_stamp = stamp.len() >= 20
        && stamp.ends_with('Z')
        && stamp.as_bytes().get(4) == Some(&b'-')
        && stamp.as_bytes().get(10) == Some(&b'T')
        && stamp[..4].bytes().all(|b| b.is_ascii_digit());
    if looks_like_stamp {
        rest
    } else {
        line
    }
}

/// The REST annotations array → [`CheckAnnotation`]s, dropping entries with
/// no message. Pure, so the shape is tested without the network.
pub(crate) fn parse_annotations(body: &Value) -> Vec<CheckAnnotation> {
    body.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    let message = a["message"]
                        .as_str()
                        .map(str::trim)
                        .filter(|m| !m.is_empty())?;
                    Some(CheckAnnotation {
                        path: a["path"].as_str().unwrap_or_default().to_string(),
                        start_line: a["start_line"].as_u64().unwrap_or_default() as u32,
                        end_line: a["end_line"].as_u64().unwrap_or_default() as u32,
                        level: a["annotation_level"]
                            .as_str()
                            .unwrap_or("notice")
                            .to_string(),
                        message: message.to_string(),
                        title: a["title"]
                            .as_str()
                            .filter(|t| !t.is_empty())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tail_keeps_the_last_lines_and_strips_runner_stamps() {
        let log = "2026-10-21T10:00:00.1234567Z ##[group]Run cargo test\n\
                   2026-10-21T10:00:01.0000000Z    Compiling foo v0.1.0\n\
                   2026-10-21T10:00:02.0000000Z error[E0425]: cannot find value `x`\n\
                   plain line without a stamp\n";
        let (tail, truncated) = tail_lines(log, 2);
        assert_eq!(
            tail,
            "error[E0425]: cannot find value `x`\nplain line without a stamp"
        );
        assert!(truncated);

        let (whole, truncated) = tail_lines(log, 10);
        assert!(!truncated);
        assert!(whole.starts_with("##[group]Run cargo test\n"));
        assert!(!whole.contains("2026-10-21T"));
    }

    #[test]
    fn tail_clamps_the_line_count() {
        let log = (0..5)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (one, _) = tail_lines(&log, 0);
        assert_eq!(one, "l4", "zero lines reads as one");
        let (all, truncated) = tail_lines(&log, CHECK_LOG_MAX_LINES + 1);
        assert_eq!(all.lines().count(), 5);
        assert!(!truncated);
    }

    #[test]
    fn a_word_that_merely_ends_in_z_is_not_a_stamp() {
        assert_eq!(
            strip_runner_timestamp("fuzz test passed"),
            "fuzz test passed"
        );
        assert_eq!(
            strip_runner_timestamp("2026-10-21T10:00:00Z ok"),
            "ok",
            "a stamp without fractional seconds still counts"
        );
        assert_eq!(strip_runner_timestamp("nospace"), "nospace");
    }

    #[test]
    fn annotations_parse_and_drop_the_messageless() {
        let body = json!([
            { "path": "src/lib.rs", "start_line": 10, "end_line": 12,
              "annotation_level": "failure", "message": "  mismatched types ",
              "title": "error[E0308]" },
            { "path": "src/x.rs", "start_line": 1, "end_line": 1,
              "annotation_level": "warning", "message": "", "title": "" },
            { "path": "README.md", "start_line": 3, "end_line": 3,
              "annotation_level": null, "message": "spelling", "title": null }
        ]);
        let got = parse_annotations(&body);
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0],
            CheckAnnotation {
                path: "src/lib.rs".into(),
                start_line: 10,
                end_line: 12,
                level: "failure".into(),
                message: "mismatched types".into(),
                title: Some("error[E0308]".into()),
            }
        );
        assert_eq!(got[1].level, "notice");
        assert_eq!(got[1].title, None);
        assert!(parse_annotations(&json!({ "message": "Not Found" })).is_empty());
    }
}
