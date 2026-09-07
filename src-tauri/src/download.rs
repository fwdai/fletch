//! One verified download for the whole app: the portable git dist, the
//! codegraph bundle, and the Whisper weights all fetch a single pinned file
//! and must never load a partial or substituted one.
//!
//! Every artifact lands in a temp file next to its destination and is hashed
//! while it streams, so a mismatch costs one pass and leaves nothing behind.
//! The temp path is returned rather than the final one: only the caller knows
//! whether "installed" means a rename or an extraction, and staging keeps the
//! final location appearing complete or absent, never half-written.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use crate::error::{Error, Result};

/// The temp file is named per-process so two Fletch instances fetching the
/// same artifact don't stream over each other.
const TMP_PREFIX: &str = "download-";
const TMP_SUFFIX: &str = ".tmp";

/// How long a download may go without receiving a byte before it fails. This
/// is what makes a temp file's age meaningful: a live download touches its
/// file at least this often, and one that stalls longer is torn down (and its
/// temp file removed) rather than left hanging on a connection the network
/// forgot. Sweeps that reclaim leftovers key their threshold off this.
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .read_timeout(READ_TIMEOUT)
            .build()
            .expect("static reqwest client")
    })
}

/// Where [`download_verified`] streams to for `dest_dir`. Exposed so a caller
/// sweeping leftovers can tell its own in-flight file from a stale one.
pub fn tmp_path(dest_dir: &Path) -> PathBuf {
    dest_dir.join(format!("{TMP_PREFIX}{}{TMP_SUFFIX}", std::process::id()))
}

/// Whether `name` is one of this module's temp files. A process killed
/// mid-download leaves one behind (the cleanup below only runs for failures we
/// live to see), and the pid in the name means it is never reused — so
/// directories that hold large artifacts sweep for these on their next run.
pub fn is_tmp_name(name: &str) -> bool {
    name.starts_with(TMP_PREFIX) && name.ends_with(TMP_SUFFIX)
}

/// Stream `url` into a temp file inside `dest_dir`, hashing as it downloads;
/// error unless the digest matches `expected_sha256`. Returns the temp path,
/// which the caller renames or unpacks into place. On any failure — transport,
/// disk, or a digest mismatch — the temp file is removed.
///
/// `on_progress(received, total)` is throttled to ~5% steps (or every 8 MiB
/// when the server sends no `Content-Length`), plus one final call once the
/// last byte is in, so a UI gets a live number without an event per network
/// chunk.
pub async fn download_verified(
    url: &str,
    expected_sha256: &str,
    dest_dir: &Path,
    on_progress: impl Fn(u64, Option<u64>),
) -> Result<PathBuf> {
    let tmp = tmp_path(dest_dir);
    match stream_verified(url, expected_sha256, &tmp, on_progress).await {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            Err(e)
        }
    }
}

async fn stream_verified(
    url: &str,
    expected_sha256: &str,
    tmp: &Path,
    on_progress: impl Fn(u64, Option<u64>),
) -> Result<()> {
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;

    let mut resp = client()
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Other(format!("download {url}: {e}")))?
        .error_for_status()
        .map_err(|e| Error::Other(format!("download {url}: {e}")))?;
    let total = resp.content_length();

    let mut file = tokio::fs::File::create(tmp)
        .await
        .map_err(|e| Error::Other(format!("create {}: {e}", tmp.display())))?;

    let mut hasher = sha2::Sha256::new();
    let mut received: u64 = 0;
    let mut last_reported: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| Error::Other(format!("download {url}: {e}")))?
    {
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| Error::Other(format!("write {}: {e}", tmp.display())))?;
        received += chunk.len() as u64;
        let step = total.map_or(8 * 1024 * 1024, |t| t / 20).max(1);
        if received - last_reported >= step {
            last_reported = received;
            on_progress(received, total);
        }
    }
    file.flush()
        .await
        .map_err(|e| Error::Other(format!("flush {}: {e}", tmp.display())))?;
    drop(file);
    on_progress(received, total);

    let digest = format!("{:x}", hasher.finalize());
    if !digest.eq_ignore_ascii_case(expected_sha256) {
        return Err(Error::Other(format!(
            "checksum mismatch for {url} (got {digest}, expected {expected_sha256})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sweep for leftovers has to recognize our temp files by name alone —
    /// and must not match the artifacts sitting in the same directory.
    #[test]
    fn tmp_names_are_recognizable() {
        let dir = Path::new("/tmp/x");
        let name = tmp_path(dir)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(is_tmp_name(&name));
        assert!(is_tmp_name("download-1.tmp"));
        assert!(!is_tmp_name("ggml-large-v3-turbo-q5_0.bin"));
        assert!(!is_tmp_name("download-1.tmp.bin"));
    }
}
