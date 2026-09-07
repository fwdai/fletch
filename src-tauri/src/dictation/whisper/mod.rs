//! Local dictation on whisper.cpp: the model catalog (`models`), getting its
//! weights onto disk (`install`), and the transcriber that runs them
//! (`engine`).
//!
//! The platform recognizer (`super::apple`) stays the default. The user opts
//! into this engine from Settings, which downloads the pinned model into
//! [`models_root`]; the dictation commands dispatch here only when the
//! setting is on AND the model is installed, so a half-finished download can
//! never leave the mic button dead.

use std::path::PathBuf;
use std::sync::OnceLock;

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

/// Where model files live: `<app data>/whisper-models`. Set once from `setup`,
/// like `git_dist::init`, so download and load paths never need an `AppHandle`.
static MODELS_ROOT: OnceLock<PathBuf> = OnceLock::new();

pub fn init(root: PathBuf) {
    let _ = MODELS_ROOT.set(root);
}

/// `None` only before `init` ran (tests, or a call from a build without setup).
pub fn models_root() -> Option<PathBuf> {
    MODELS_ROOT.get().cloned()
}
