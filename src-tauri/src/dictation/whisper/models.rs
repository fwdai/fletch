//! The Whisper model catalog. Weights are pinned here, not fetched from a
//! listing: each entry names the exact file, its SHA-256, and its size, so a
//! download is verified before it is ever loaded and a swap is a one-line
//! change to [`DEFAULT_MODEL_ID`] (plus a new entry) when a better model ships.
//!
//! Files come from the ggml conversions at
//! <https://huggingface.co/ggerganov/whisper.cpp>. To add one, take `oid
//! sha256` and `size` verbatim from the LFS pointer at
//! `https://huggingface.co/ggerganov/whisper.cpp/raw/main/<file>`.

use std::path::PathBuf;

/// One downloadable model. `id` is the stable key stored in settings and shown
/// to the frontend; `label`/`note` are display copy.
#[derive(Clone, Copy, Debug)]
pub struct WhisperModel {
    pub id: &'static str,
    pub label: &'static str,
    /// One line on when to pick it (shown under the label in Settings).
    pub note: &'static str,
    pub file_name: &'static str,
    pub url: &'static str,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: &'static str,
    /// Exact byte size, for progress and the cheap "is it installed" check.
    pub size: u64,
}

/// What a fresh opt-in downloads. Swap by pointing at another catalog entry.
pub const DEFAULT_MODEL_ID: &str = "large-v3-turbo-q5_0";

pub const MODELS: &[WhisperModel] = &[
    WhisperModel {
        id: "large-v3-turbo-q5_0",
        label: "Large v3 Turbo (5-bit)",
        note: "Best accuracy per second on Apple Silicon. Multilingual.",
        file_name: "ggml-large-v3-turbo-q5_0.bin",
        url:
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        size: 574_041_195,
    },
    WhisperModel {
        id: "small.en-q8_0",
        label: "Small English (8-bit)",
        note: "Much faster on Intel Macs. English only.",
        file_name: "ggml-small.en-q8_0.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en-q8_0.bin",
        sha256: "67a179f608ea6114bd3fdb9060e762b588a3fb3bd00c4387971be4d177958067",
        size: 264_477_561,
    },
];

pub fn default_model() -> &'static WhisperModel {
    find(DEFAULT_MODEL_ID).expect("DEFAULT_MODEL_ID names a catalog entry")
}

pub fn find(id: &str) -> Option<&'static WhisperModel> {
    MODELS.iter().find(|m| m.id == id)
}

/// Where this model lives once installed: `<models_root>/<file_name>`.
/// `None` before `whisper::init`.
pub fn path(model: &WhisperModel) -> Option<PathBuf> {
    super::models_root().map(|root| root.join(model.file_name))
}

/// The model's file, if it is fully present. Checks the exact byte size, not
/// the hash: the download path verifies the digest before moving the file into
/// place, so a file of the right size at this path is one that passed.
pub fn installed_path(model: &WhisperModel) -> Option<PathBuf> {
    let p = path(model)?;
    let len = std::fs::metadata(&p).ok()?.len();
    (len == model.size).then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_in_catalog() {
        assert_eq!(default_model().id, DEFAULT_MODEL_ID);
    }

    #[test]
    fn catalog_entries_are_well_formed() {
        for m in MODELS {
            assert_eq!(m.sha256.len(), 64, "{}: sha256 must be 32 bytes hex", m.id);
            assert!(m.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{}", m.id);
            assert!(
                m.url.ends_with(m.file_name),
                "{}: url must end in file_name",
                m.id
            );
            assert!(m.size > 0, "{}", m.id);
        }
    }
}
