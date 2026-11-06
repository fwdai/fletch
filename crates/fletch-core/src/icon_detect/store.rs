//! The project's chosen icon. `icon.source` in `project_settings` records the
//! choice — `detected:<path>`, `file`, `builtin:<glyph>` or `initial` — and no
//! row means nothing was decided yet, which detection fills in (on add,
//! relocate and the startup pass). The image itself is copied once to
//! `<data dir>/project-icons/<project id>`, so drawing it never reads the
//! repository and a relocated repo keeps its icon. `icon.hash`, the host's own
//! row, is what `ProjectRef.icon_hash` carries: a client refetches the image
//! only when it changes.

use std::path::{Path, PathBuf};

use base64::Engine;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{image, scan_project, Candidate, MAX_BYTES};
use crate::error::{Error, Result};

/// The choice, a client-readable project setting written by `project_icon_set`.
pub const SOURCE_KEY: &str = "icon.source";
/// The stored image's hash. The host's own: never answered, never writable.
pub const HASH_KEY: &str = "icon.hash";
/// The built-in glyphs the picker offers; the client maps each to its own icon.
pub const GLYPHS: &[&str] = &["web", "desktop", "mobile", "cli", "library", "docs"];
/// The initial, chosen — by the user, or by a scan that found nothing — so
/// detection leaves it alone.
pub const INITIAL: &str = "initial";
const DIR: &str = "project-icons";

/// What `project_icon_get` answers: a built-in glyph, or an image's bytes.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum IconData {
    Glyph { glyph: String },
    Image { mime: &'static str, data: String },
}

/// A candidate with its image, for the picker's thumbnails.
#[derive(Debug, Serialize)]
pub struct CandidateImage {
    #[serde(flatten)]
    pub candidate: Candidate,
    pub mime: &'static str,
    pub data: String,
}

/// Every candidate in `repos` that holds a drawable image, best first.
pub fn candidates(repos: &[PathBuf]) -> Vec<CandidateImage> {
    scan_project(repos)
        .into_iter()
        .filter_map(|candidate| {
            let (bytes, mime) = read_icon(&candidate.file)?;
            Some(CandidateImage {
                candidate,
                mime,
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            })
        })
        .collect()
}

/// Make `source` the project's icon. `detected:<path>` must be one of the
/// scan's candidates in `repos` now — a client names a candidate, never a
/// file — and `file` takes the uploaded `bytes`. `None` clears the choice.
pub fn set(
    db: &crate::DbState,
    project_id: &str,
    source: Option<&str>,
    upload: Option<Vec<u8>>,
    repos: &[PathBuf],
) -> Result<()> {
    let image = match source {
        None | Some(INITIAL) => None,
        Some(s)
            if s.strip_prefix("builtin:")
                .is_some_and(|g| GLYPHS.contains(&g)) =>
        {
            None
        }
        Some("file") => {
            let bytes = upload.ok_or_else(|| Error::Other("no file to use as the icon".into()))?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(Error::Other("that image is larger than 1 MB".into()));
            }
            Some(
                image::normalize(bytes)
                    .ok_or_else(|| Error::Other("that file is not an image".into()))?
                    .0,
            )
        }
        Some(s) if s.starts_with("detected:") => {
            let path = &s["detected:".len()..];
            let found = scan_project(repos)
                .into_iter()
                .find(|c| c.path == path)
                .ok_or_else(|| Error::Other(format!("no icon at {path} any more")))?;
            Some(
                read_icon(&found.file)
                    .ok_or_else(|| Error::Other(format!("{path} is not an image")))?
                    .0,
            )
        }
        Some(s) => return Err(Error::Other(format!("`{s}` is not an icon choice"))),
    };
    let conn = db.lock();
    let exists: bool = conn
        .query_row("SELECT 1 FROM projects WHERE id = ?1", [project_id], |_| {
            Ok(())
        })
        .optional()?
        .is_some();
    if !exists {
        return Err(Error::Other(format!("project not found: {project_id}")));
    }
    write(&conn, project_id, source, image)
}

/// The project's icon as a client draws it; `None` for the initial.
pub fn get(db: &crate::DbState, project_id: &str) -> Result<Option<IconData>> {
    let (source, file) = {
        let conn = db.lock();
        (
            setting(&conn, project_id, SOURCE_KEY)?,
            icon_file(&conn, project_id),
        )
    };
    let Some(source) = source.filter(|s| s != INITIAL) else {
        return Ok(None);
    };
    if let Some(glyph) = source.strip_prefix("builtin:") {
        return Ok(Some(IconData::Glyph {
            glyph: glyph.to_string(),
        }));
    }
    let Some(bytes) = file.and_then(|f| std::fs::read(f).ok()) else {
        return Ok(None);
    };
    Ok(image::mime_of(&bytes).map(|mime| IconData::Image {
        mime,
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
    }))
}

/// Pick the scan's top candidate when the project has no icon choice yet: on
/// add, relocate and the startup pass. A scan that finds nothing records the
/// initial, so it is not repeated. Best-effort — a failure is logged. Whether
/// the project got an icon.
pub fn detect_if_unset(db: &crate::DbState, project_id: &str, repos: &[PathBuf]) -> bool {
    let unset = |conn: &Connection| matches!(setting(conn, project_id, SOURCE_KEY), Ok(None));
    if !unset(&db.lock()) {
        return false;
    }
    let found = scan_project(repos)
        .into_iter()
        .find_map(|c| Some((format!("detected:{}", c.path), read_icon(&c.file)?.0)));
    let conn = db.lock();
    if !unset(&conn) {
        return false;
    }
    let got_icon = found.is_some();
    let (source, image) = match found {
        Some((source, bytes)) => (source, Some(bytes)),
        None => (INITIAL.to_string(), None),
    };
    match write(&conn, project_id, Some(&source), image) {
        Ok(()) => got_icon,
        Err(e) => {
            tracing::warn!(error = %e, project_id, "couldn't store the detected project icon");
            false
        }
    }
}

/// Drop a deleted project's stored image.
pub fn forget(conn: &Connection, project_id: &str) {
    if let Some(file) = icon_file(conn, project_id) {
        let _ = std::fs::remove_file(file);
    }
}

/// Store the choice: the image file (or none), its hash, the source row.
fn write(
    conn: &Connection,
    project_id: &str,
    source: Option<&str>,
    image: Option<Vec<u8>>,
) -> Result<()> {
    let file = icon_file(conn, project_id)
        .ok_or_else(|| Error::Other("this host has no data directory for icons".into()))?;
    let hash = match (&image, source) {
        (Some(bytes), _) => {
            std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))?;
            std::fs::write(&file, bytes)?;
            Some(short_hash(bytes))
        }
        // A glyph is fetched like an image; the initial needs no fetch.
        (None, source) => {
            let _ = std::fs::remove_file(&file);
            source
                .filter(|s| *s != INITIAL)
                .map(|s| short_hash(s.as_bytes()))
        }
    };
    for (key, value) in [(HASH_KEY, hash.as_deref()), (SOURCE_KEY, source)] {
        match value {
            Some(v) => conn.execute(
                "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
                rusqlite::params![project_id, key, v],
            )?,
            None => conn.execute(
                "DELETE FROM project_settings WHERE project_id = ?1 AND key = ?2",
                rusqlite::params![project_id, key],
            )?,
        };
    }
    Ok(())
}

fn setting(conn: &Connection, project_id: &str, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
            [project_id, key],
            |r| r.get(0),
        )
        .optional()?)
}

/// `<data dir>/project-icons/<project id>`, beside the database this engine
/// opened (so a headless host's `--data-dir` and a test's temp dir both
/// hold). `None` for an in-memory database or an id that is not one.
fn icon_file(conn: &Connection, project_id: &str) -> Option<PathBuf> {
    let safe = !project_id.is_empty()
        && project_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let db = conn.path().filter(|p| !p.is_empty() && safe)?;
    Some(Path::new(db).parent()?.join(DIR).join(project_id))
}

/// A candidate's bytes as the icon to store.
fn read_icon(file: &Path) -> Option<(Vec<u8>, &'static str)> {
    if std::fs::metadata(file).ok()?.len() > MAX_BYTES {
        return None;
    }
    image::normalize(std::fs::read(file).ok()?)
}

fn short_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ctx::test_ctx;
    use crate::run_detect::test_support::fixture;

    fn project(ctx: &crate::host::EngineCtx) {
        ctx.db
            .lock()
            .execute(
                "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'p', 0)",
                [],
            )
            .unwrap();
    }

    fn hash(ctx: &crate::host::EngineCtx) -> Option<String> {
        setting(&ctx.db.lock(), "p1", HASH_KEY).unwrap()
    }

    #[test]
    fn detection_fills_an_unset_icon_and_every_choice_round_trips() {
        let (ctx, _sink, _dir) = test_ctx();
        project(&ctx);
        let repo = fixture(&[
            ("public/favicon.svg", "<svg id='a'/>"),
            ("docs/logo.svg", "<svg/>"),
        ]);
        let repos = [repo.path().to_path_buf()];

        detect_if_unset(&ctx.db, "p1", &repos);
        assert_eq!(
            setting(&ctx.db.lock(), "p1", SOURCE_KEY)
                .unwrap()
                .as_deref(),
            Some("detected:public/favicon.svg")
        );
        let detected = hash(&ctx).unwrap();
        assert_eq!(
            get(&ctx.db, "p1").unwrap(),
            Some(IconData::Image {
                mime: "image/svg+xml",
                data: base64::engine::general_purpose::STANDARD.encode("<svg id='a'/>"),
            })
        );

        // The repository can change; the stored copy does not.
        std::fs::remove_file(repo.path().join("public/favicon.svg")).unwrap();
        assert!(get(&ctx.db, "p1").unwrap().is_some());

        set(&ctx.db, "p1", Some("builtin:cli"), None, &repos).unwrap();
        assert_eq!(
            get(&ctx.db, "p1").unwrap(),
            Some(IconData::Glyph {
                glyph: "cli".into()
            })
        );
        assert_ne!(hash(&ctx).unwrap(), detected);

        set(
            &ctx.db,
            "p1",
            Some("file"),
            Some(b"<svg/>".to_vec()),
            &repos,
        )
        .unwrap();
        assert!(matches!(
            get(&ctx.db, "p1").unwrap(),
            Some(IconData::Image { .. })
        ));

        // "Use initial" is a choice detection leaves alone (a relocate, the
        // startup pass); only a cleared choice is detected again.
        set(&ctx.db, "p1", Some(INITIAL), None, &repos).unwrap();
        assert_eq!((get(&ctx.db, "p1").unwrap(), hash(&ctx)), (None, None));
        detect_if_unset(&ctx.db, "p1", &repos);
        assert_eq!(get(&ctx.db, "p1").unwrap(), None);
        set(&ctx.db, "p1", None, None, &repos).unwrap();
        detect_if_unset(&ctx.db, "p1", &repos);
        assert!(get(&ctx.db, "p1").unwrap().is_some());
    }

    /// A scan that finds nothing records the initial, so the next boot's pass
    /// does not scan the project again.
    #[test]
    fn a_scan_that_finds_nothing_is_not_repeated() {
        let (ctx, _sink, _dir) = test_ctx();
        project(&ctx);
        let repo = fixture(&[("README.md", "# no icon")]);
        detect_if_unset(&ctx.db, "p1", &[repo.path().to_path_buf()]);
        assert_eq!(
            setting(&ctx.db.lock(), "p1", SOURCE_KEY)
                .unwrap()
                .as_deref(),
            Some(INITIAL)
        );
        assert_eq!((get(&ctx.db, "p1").unwrap(), hash(&ctx)), (None, None));
    }

    #[test]
    fn a_client_names_a_candidate_never_a_file() {
        let (ctx, _sink, _dir) = test_ctx();
        project(&ctx);
        let repo = fixture(&[("docs/logo.svg", "<svg/>"), ("secret.txt", "<svg")]);
        let repos = [repo.path().to_path_buf()];
        for bad in [
            "detected:secret.txt",
            "detected:../x.svg",
            "builtin:rocket",
            "nope",
        ] {
            assert!(
                set(&ctx.db, "p1", Some(bad), None, &repos).is_err(),
                "{bad}"
            );
        }
        assert!(set(
            &ctx.db,
            "p1",
            Some("file"),
            Some(b"plain text".to_vec()),
            &repos
        )
        .is_err());
        assert!(set(&ctx.db, "missing", Some("builtin:web"), None, &repos).is_err());
        set(&ctx.db, "p1", Some("detected:docs/logo.svg"), None, &repos).unwrap();
        assert_eq!(candidates(&repos).len(), 1);
    }
}
