//! A project's `.fletch/settings.json` (`crate::project_file`): what it says
//! and what of it applies, and the user's approval of what it adds. Shared by
//! the Tauri commands and the remote dispatcher.

use serde::Serialize;

use crate::error::Result;
use crate::host::EngineCtx;
use crate::project_file::{self, Edit, ProjectFileStatus, Which, EVENT_CHANGED};

#[derive(Serialize)]
struct Changed<'a> {
    project_id: &'a str,
}

/// `project_file_status(project_id)`: the files re-read now, so the answer is
/// current even between the watcher's looks.
pub fn project_file_status_impl(ctx: &EngineCtx, project_id: &str) -> Result<ProjectFileStatus> {
    let (status, changed) = project_file::refresh(&ctx.db.lock(), project_id);
    if changed {
        emit_changed(ctx, project_id);
    }
    Ok(status)
}

/// `project_file_approve(project_id, patterns)`: approve host patterns the
/// files add, so the project's boxes may reach them. Refused for a pattern the
/// files do not hold now.
pub fn project_file_approve_impl(
    ctx: &EngineCtx,
    project_id: &str,
    patterns: &[String],
) -> Result<ProjectFileStatus> {
    let status = project_file::approve(&ctx.db.lock(), project_id, patterns)?;
    emit_changed(ctx, project_id);
    Ok(status)
}

/// `project_file_edit(project_id, file, edit)`: change the `egress` lists of
/// one of the project's settings files from the app — `shared` is the team's
/// `.fletch/settings.json`, `local` the user's own. Hosts added this way are
/// approved as they are written. Refused for a file with comments.
pub fn project_file_edit_impl(
    ctx: &EngineCtx,
    project_id: &str,
    file: Which,
    edit: &Edit,
) -> Result<ProjectFileStatus> {
    let status = project_file::edit(&ctx.db.lock(), project_id, file, edit)?;
    emit_changed(ctx, project_id);
    Ok(status)
}

fn emit_changed(ctx: &EngineCtx, project_id: &str) {
    crate::host::emit(ctx.sink.as_ref(), EVENT_CHANGED, &Changed { project_id });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ctx::test_ctx;

    /// The commands read the project's primary repo, refuse to approve what
    /// its files do not say, and apply what they approve.
    #[test]
    fn status_and_approval_round_trip() {
        let (ctx, sink, _dir) = test_ctx();
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        std::fs::create_dir_all(repo.join(".fletch")).unwrap();
        std::fs::write(
            repo.join(".fletch/settings.json"),
            r#"{ "egress": { "allow": ["static.rust-lang.org"] } }"#,
        )
        .unwrap();
        {
            let conn = ctx.db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, created_at) VALUES ('pc', 'p', 0)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO repos (id, project_id, path, created_at) VALUES ('rc', 'pc', ?1, 0)",
                [repo.to_string_lossy()],
            )
            .unwrap();
        }

        let status = project_file_status_impl(&ctx, "pc").unwrap();
        assert_eq!(status.egress.pending, vec!["static.rust-lang.org"]);
        assert!(project_file_approve_impl(&ctx, "pc", &["example.com".into()]).is_err());
        let status =
            project_file_approve_impl(&ctx, "pc", &["static.rust-lang.org".into()]).unwrap();
        assert_eq!(status.egress.applied, vec!["static.rust-lang.org"]);
        assert!(status.egress.pending.is_empty());
        assert!(sink
            .events()
            .iter()
            .any(|(event, payload)| event == EVENT_CHANGED && payload["project_id"] == "pc"));
    }
}
