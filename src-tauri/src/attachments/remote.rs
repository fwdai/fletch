//! Attachments from a paired phone: the phone picks a photo or file, streams
//! its bytes over the remote protocol, and this Mac stages them exactly where
//! the desktop composer stages a pasted screenshot — so `send_user_message`
//! adopts them into the agent's workspace with no path of its own.
//!
//! The wire shape is four ops (`docs/remote-protocol.md`, "Attachments"),
//! shaped like dictation's: `attachment_begin` opens an upload and names the
//! file, `attachment_chunk` appends bytes, `attachment_end` closes it and
//! answers with the staged path the phone then puts in `attachments` on its
//! message, and `attachment_cancel` throws the upload away.
//!
//! Chunked because a device WebSocket message is capped at 4 MiB and a phone
//! screenshot, base64-inflated, is often over it. Written straight to disk as
//! the chunks arrive rather than buffered: a photo is tens of megabytes, and
//! eight of them in memory across every phone is not a bound worth having.
//! Everything else is bounded — bytes per upload, uploads open at once, and an
//! idle sweep for a phone that vanished mid-upload — and a swept or cancelled
//! upload takes its staging dir with it.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base64::Engine as _;
use parking_lot::Mutex;
use serde::Serialize;

use crate::error::{Error, Result};

/// Uploads that may be open at once, across every paired phone. A user attaches
/// a handful of screenshots to one message; this is a bound on abuse, not use.
pub const MAX_UPLOADS: usize = 8;

/// How long an upload may go without a chunk before it is swept. A phone that
/// loses its connection mid-upload never sends `attachment_end`, and what it
/// left behind is a partial file in the staging area.
pub const UPLOAD_IDLE: Duration = Duration::from_secs(120);

/// Largest decoded chunk accepted. The phone sends 1 MiB slices (which base64
/// to about 1.4 MiB, well under the 4 MiB frame cap); anything near the cap
/// itself is a client that is not chunking.
pub const MAX_CHUNK_BYTES: usize = 2 * 1024 * 1024;

/// Largest file accepted, decoded. Room for a full-resolution phone photo; the
/// phone re-encodes anything larger before sending.
pub const MAX_UPLOAD_BYTES: u64 = 32 * 1024 * 1024;

/// The name a file with no usable name is staged under.
const FALLBACK_NAME: &str = "attachment";

#[derive(Serialize)]
pub struct Begun {
    pub upload: String,
}

#[derive(Serialize)]
pub struct Ended {
    /// The staged path, to go in `send_user_message`'s `attachments`. Under the
    /// app-data dir, so an agent cannot read it until `adopt` moves it.
    pub path: String,
}

struct Upload {
    /// `<root>/<uuid>/<name>` — the exact shape `adopt_one` accepts.
    path: PathBuf,
    file: File,
    written: u64,
    touched: Instant,
}

/// The open uploads. Separate from the global so the tests can drive one of
/// their own, rooted in a temp dir and with a clock they control.
pub(super) struct Store {
    root: PathBuf,
    uploads: Mutex<HashMap<String, Upload>>,
}

/// The process-wide store the ops act on. `OnceLock` rather than `LazyLock`:
/// the crate's MSRV predates the latter.
static STORE: OnceLock<Store> = OnceLock::new();

fn store() -> &'static Store {
    STORE.get_or_init(|| Store::new(super::staging_root()))
}

impl Store {
    pub(super) fn new(root: PathBuf) -> Self {
        Self {
            root,
            uploads: Mutex::new(HashMap::new()),
        }
    }

    /// Open an upload for a file called `name`. Sweeps idle ones first, so a
    /// phone that dropped out never blocks the next one from starting.
    pub(super) fn begin(&self, name: &str, now: Instant) -> Result<String> {
        let mut uploads = self.uploads.lock();
        let stale: Vec<String> = uploads
            .iter()
            .filter(|(_, u)| now.duration_since(u.touched) >= UPLOAD_IDLE)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some(upload) = uploads.remove(&id) {
                discard(&upload.path);
            }
        }
        if uploads.len() >= MAX_UPLOADS {
            return Err(Error::Other(
                "Too many attachments are uploading to your Mac at once. Try again in a moment."
                    .into(),
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let dir = self.root.join(&id);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(super::sanitize_name(name, FALLBACK_NAME));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .inspect_err(|_| discard(&path))?;
        uploads.insert(
            id.clone(),
            Upload {
                path,
                file,
                written: 0,
                touched: now,
            },
        );
        Ok(id)
    }

    /// Append one decoded chunk. Past the size cap the upload is refused *and*
    /// dropped: unlike dictation, a truncated file is not a lesser version of
    /// the same thing, it is a corrupt one.
    pub(super) fn append(&self, id: &str, bytes: &[u8], now: Instant) -> Result<()> {
        let mut uploads = self.uploads.lock();
        let upload = uploads.get_mut(id).ok_or_else(unknown_upload)?;
        let total = upload.written + bytes.len() as u64;
        if total > MAX_UPLOAD_BYTES {
            let upload = uploads.remove(id).expect("looked up above");
            discard(&upload.path);
            return Err(Error::Other(format!(
                "Attachment is over the {} MB limit.",
                MAX_UPLOAD_BYTES / (1024 * 1024)
            )));
        }
        if let Err(e) = upload.file.write_all(bytes) {
            let upload = uploads.remove(id).expect("looked up above");
            discard(&upload.path);
            return Err(e.into());
        }
        upload.written = total;
        upload.touched = now;
        Ok(())
    }

    /// Close the upload and hand back its staged path. An upload that never
    /// received a chunk is an empty file, which is still a file the agent can
    /// be told about — the phone decides whether that is worth sending.
    pub(super) fn end(&self, id: &str) -> Result<PathBuf> {
        let upload = self.uploads.lock().remove(id).ok_or_else(unknown_upload)?;
        upload.file.sync_all()?;
        Ok(upload.path)
    }

    /// Drop an upload and its partial file. Idempotent: a cancel for one that
    /// was already swept or ended has nothing left to do.
    pub(super) fn cancel(&self, id: &str) {
        if let Some(upload) = self.uploads.lock().remove(id) {
            discard(&upload.path);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.uploads.lock().len()
    }
}

/// Remove a staged file and the UUID dir around it. Best-effort: the sweep
/// that finds a leftover later is the backstop.
fn discard(path: &Path) {
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir(dir);
    }
}

fn unknown_upload() -> Error {
    Error::Other("attachment: unknown upload — it may have timed out; attach it again".into())
}

fn decode(data_base64: &str) -> Result<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64)
        .map_err(|e| Error::Other(format!("attachment: chunk is not base64: {e}")))?;
    if bytes.len() > MAX_CHUNK_BYTES {
        return Err(Error::Other(format!(
            "attachment: chunk of {} bytes is over the {} byte limit",
            bytes.len(),
            MAX_CHUNK_BYTES
        )));
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// The ops, as the dispatcher calls them.

pub fn begin(name: &str) -> Result<Begun> {
    Ok(Begun {
        upload: store().begin(name, Instant::now())?,
    })
}

pub fn append(upload: &str, data_base64: &str) -> Result<()> {
    let bytes = decode(data_base64)?;
    store().append(upload, &bytes, Instant::now())
}

pub fn end(upload: &str) -> Result<Ended> {
    Ok(Ended {
        path: store().end(upload)?.to_string_lossy().into_owned(),
    })
}

pub fn cancel(upload: &str) {
    store().cancel(upload);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        (dir, store)
    }

    #[test]
    fn chunks_land_in_order_at_the_staging_shape_adopt_expects() {
        let (root, store) = store();
        let now = Instant::now();
        let id = store.begin("shot.png", now).unwrap();
        store.append(&id, b"pix", now).unwrap();
        store.append(&id, b"els", now).unwrap();

        let path = store.end(&id).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"pixels");
        assert_eq!(path.file_name().unwrap(), "shot.png");
        // `<root>/<uuid>/<name>`: exactly one dir below the root, so `adopt`
        // will move it into the agent's workspace like a desktop paste.
        assert_eq!(path.parent().unwrap().parent().unwrap(), root.path());
        assert!(store.end(&id).is_err(), "an upload ends once");
        assert_eq!(store.len(), 0);

        let workspace = tempfile::tempdir().unwrap();
        let adopted = super::super::adopt_into(
            root.path(),
            workspace.path(),
            &[path.to_string_lossy().into_owned()],
        );
        assert!(Path::new(&adopted[0]).starts_with(workspace.path()));
        assert_eq!(std::fs::read(&adopted[0]).unwrap(), b"pixels");
    }

    #[test]
    fn a_path_shaped_name_cannot_leave_its_staging_dir() {
        let (root, store) = store();
        let now = Instant::now();
        for (name, expect) in [
            ("../../fletch.db", "fletch.db"),
            ("/etc/passwd", "passwd"),
            ("", FALLBACK_NAME),
            ("..", FALLBACK_NAME),
            ("photos/IMG_0001.HEIC", "IMG_0001.HEIC"),
        ] {
            let id = store.begin(name, now).unwrap();
            let path = store.end(&id).unwrap();
            assert_eq!(path.file_name().unwrap(), expect, "{name:?}");
            assert_eq!(path.parent().unwrap().parent().unwrap(), root.path());
        }
    }

    #[test]
    fn an_upload_over_the_cap_is_refused_and_removed() {
        let (root, store) = store();
        let now = Instant::now();
        let id = store.begin("big.bin", now).unwrap();
        // Bring it right to the cap, then one byte over.
        let slice = vec![0u8; MAX_CHUNK_BYTES];
        let full = MAX_UPLOAD_BYTES as usize / MAX_CHUNK_BYTES;
        for _ in 0..full {
            store.append(&id, &slice, now).unwrap();
        }
        assert!(store.append(&id, &[1], now).is_err());
        assert!(store.end(&id).is_err(), "the upload is gone with its file");
        assert!(
            std::fs::read_dir(root.path()).unwrap().next().is_none(),
            "nothing is left in staging"
        );
    }

    #[test]
    fn cancel_removes_the_partial_file_and_is_idempotent() {
        let (root, store) = store();
        let now = Instant::now();
        let id = store.begin("shot.png", now).unwrap();
        store.append(&id, b"half", now).unwrap();
        store.cancel(&id);
        store.cancel(&id);
        assert!(store.end(&id).is_err());
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[test]
    fn idle_uploads_are_swept_with_their_files_and_live_ones_capped() {
        let (root, store) = store();
        let t0 = Instant::now();
        let stale = store.begin("old.png", t0).unwrap();
        store.append(&stale, b"x", t0).unwrap();
        // The rest start half an idle period later, so they are still live
        // when the stale one has aged out.
        let t1 = t0 + UPLOAD_IDLE / 2;
        for _ in 1..MAX_UPLOADS {
            store.begin("live.png", t1).unwrap();
        }
        assert!(
            store.begin("one-more.png", t1).is_err(),
            "the cap holds while all are live"
        );

        let later = t0 + UPLOAD_IDLE;
        let fresh = store.begin("new.png", later).unwrap();
        assert!(
            store.append(&stale, b"y", later).is_err(),
            "the idle upload was swept to make room"
        );
        store.append(&fresh, b"z", later).unwrap();
        // The swept upload's dir went with it; the live ones' dirs remain.
        let dirs = std::fs::read_dir(root.path()).unwrap().count();
        assert_eq!(dirs, MAX_UPLOADS);
    }

    #[test]
    fn chunks_are_base64_and_bounded() {
        assert_eq!(decode("AAEA/w==").unwrap(), [0, 1, 0, 255]);
        assert!(decode("not base64!").is_err());
        let over = base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_CHUNK_BYTES + 2]);
        assert!(decode(&over).is_err());
    }
}
