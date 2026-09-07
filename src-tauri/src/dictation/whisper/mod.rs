//! Local dictation on whisper.cpp: the model catalog (`models`), getting its
//! weights onto disk (`install`), and the transcriber that runs them
//! (`engine`).
//!
//! The platform recognizer (`super::apple`) stays the default. The user opts
//! into this engine from Settings, which downloads the chosen model into
//! [`models_root`]; the dictation commands dispatch here only when the
//! setting is on AND that model is installed, so a half-finished download can
//! never leave the mic button dead.

use std::path::PathBuf;
use std::sync::OnceLock;

use rusqlite::Connection;

use crate::database;
use crate::DbState;

// whisper.cpp itself is built for macOS only (iOS keeps the platform
// recognizer), so the catalog and install compile everywhere but the
// transcriber doesn't.
#[cfg(target_os = "macos")]
pub mod engine;
pub mod install;
pub mod models;

/// `settings` key for the engine choice. Only [`ENGINE_WHISPER`] selects the
/// local engine; anything else means the platform recognizer — written as
/// [`ENGINE_APPLE`] rather than cleared, so an opt-out is a recorded choice.
pub const ENGINE_SETTING: &str = "dictation_engine";
pub const ENGINE_WHISPER: &str = "whisper";
pub const ENGINE_APPLE: &str = "apple";

/// Interpret the raw setting value. Opt-in: only an explicit `"whisper"` is on.
pub fn parse_enabled(raw: Option<&str>) -> bool {
    raw == Some(ENGINE_WHISPER)
}

/// `settings` key for which catalog entry the engine uses, by
/// [`models::WhisperModel::id`]. Absent until the user picks one, which is not
/// the same as "no model": see [`selected_model`].
pub const MODEL_SETTING: &str = "dictation_model";

/// Interpret the raw setting value. An id that isn't in the catalog is treated
/// as absent rather than as an error — a model dropped from `MODELS` in an
/// update must not leave dictation unable to name a model at all.
pub fn selected_model(raw: Option<&str>) -> &'static models::WhisperModel {
    raw.and_then(models::find)
        .unwrap_or_else(models::platform_default)
}

/// The chosen model, for a caller that already holds the connection. The one
/// path the commands and `dictation::engine` share, so the status a Settings
/// row shows and the weights a session loads can't disagree.
pub fn selected(conn: &Connection) -> &'static models::WhisperModel {
    selected_model(database::get_setting(conn, MODEL_SETTING).as_deref())
}

/// Where model files live: `<app data>/whisper-models`. Set once from `setup`,
/// like `git_dist::init`, so download and load paths never need an `AppHandle`.
static MODELS_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// The database, for the one caller that reads the selection without a
/// connection of its own — the engine loading weights mid-session, reached
/// from the mic tap's teardown rather than from a command.
static DB: OnceLock<DbState> = OnceLock::new();

pub fn init(root: PathBuf, db: DbState) {
    let _ = MODELS_ROOT.set(root);
    let _ = DB.set(db);
}

/// The chosen model, taking the database lock itself. A caller already holding
/// the connection must use [`selected`] instead — this one would deadlock.
/// Before `init` (tests) it answers with the platform default.
pub fn selected_now() -> &'static models::WhisperModel {
    match DB.get() {
        Some(db) => selected(&db.lock()),
        None => models::platform_default(),
    }
}

/// `None` only before `init` ran (tests, or a call from a build without setup).
pub fn models_root() -> Option<PathBuf> {
    MODELS_ROOT.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_choice_wins_and_anything_else_falls_back() {
        assert_eq!(
            selected_model(Some(models::SMALL_MODEL_ID)).id,
            models::SMALL_MODEL_ID
        );
        let fallback = models::platform_default().id;
        assert_eq!(selected_model(None).id, fallback);
        assert_eq!(selected_model(Some("tiny.en")).id, fallback);
        assert_eq!(selected_model(Some("")).id, fallback);
    }
}
