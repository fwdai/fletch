//! A project's icon (docs/project-surfaces-plan.md §3.6): what the scan finds
//! in its repositories, the choice, and the stored image — the same `_impl`s
//! for the Tauri commands and the remote dispatcher.

use base64::Engine;

use crate::error::{Error, Result};
use crate::host::EngineCtx;
use crate::icon_detect::{self, CandidateImage, IconData};
use crate::supervisor::Supervisor;
use crate::workspace::Workspace;

/// The candidates in the project's repositories, best first, with their images
/// for the picker. A fresh scan every time: this is "Detect again".
pub fn project_icon_candidates_impl(sup: &Supervisor, project_id: &str) -> Vec<CandidateImage> {
    icon_detect::candidates(&sup.workspace.project_repo_paths(project_id))
}

/// Make `source` the project's icon (`bytes`, base64, for `file`), then tell
/// every view: the answer and `workspace:changed` carry the new `icon_hash`,
/// and `project_settings:changed` the new `icon.source`.
pub fn project_icon_set_impl(
    ctx: &EngineCtx,
    sup: &Supervisor,
    project_id: &str,
    source: Option<&str>,
    bytes: Option<&str>,
) -> Result<Workspace> {
    let upload = bytes
        .map(|b| base64::engine::general_purpose::STANDARD.decode(b))
        .transpose()
        .map_err(|e| Error::Other(format!("bad icon bytes: {e}")))?;
    let repos = sup.workspace.project_repo_paths(project_id);
    icon_detect::set(&ctx.db, project_id, source, upload, &repos)?;
    crate::host::emit(
        ctx.sink.as_ref(),
        super::PROJECT_SETTINGS_CHANGED,
        &serde_json::json!({ "project_id": project_id, "key": icon_detect::SOURCE_KEY, "value": source }),
    );
    Ok(sup.workspace.current().expect("workspace initialized"))
}

/// The project's icon to draw: a glyph, an image, or `None` for the initial.
pub fn project_icon_get_impl(ctx: &EngineCtx, project_id: &str) -> Result<Option<IconData>> {
    icon_detect::get(&ctx.db, project_id)
}
