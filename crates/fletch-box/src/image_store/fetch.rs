//! Getting one image file onto disk: a whole streaming download, or a delta
//! assembled from older copies plus HTTP range requests. Either way the file
//! ends up verified against the release manifest's size and sha256.

use std::collections::HashMap;
use std::fs::{self, File};
use std::ops::ControlFlow;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::header::RANGE;
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::chunks::{self, ChunkList};
use super::{file_matches, Progress};
use crate::error::{Error, Result};

/// Two missing ranges closer than this are fetched as one: a second request
/// costs more than the bytes between them.
const COALESCE_GAP: u64 = 64 * 1024;

/// What a file is expected to be: where it lives and what the manifest says.
pub(super) struct Want<'a> {
    pub url: &'a str,
    pub size: u64,
    pub sha256: &'a str,
}

/// How a file was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Cloned from a `file://` release.
    Copied,
    /// Downloaded whole.
    Downloaded,
    /// Assembled: `copied` bytes from older copies, `fetched` by range requests.
    Delta { copied: u64, fetched: u64 },
}

pub(super) fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| Error::Other(e.to_string()))
}

/// A response body of at most `max` bytes.
pub(super) async fn read_body(
    mut response: reqwest::Response,
    max: u64,
    url: &str,
) -> Result<Vec<u8>> {
    let http = |e: reqwest::Error| Error::Http {
        url: url.to_owned(),
        message: e.to_string(),
    };
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(http)? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() as u64 > max {
            return Err(Error::Manifest {
                url: url.to_owned(),
                message: format!("larger than {max} bytes"),
            });
        }
    }
    Ok(bytes)
}

/// Puts the file `want` describes at `path` (which must not exist), verified.
/// A `file://` URL is cloned. With `seeds` (older copies of the same image on
/// this host) and a chunk list in the release, only the changed chunks are
/// downloaded; any trouble on that path other than a cancel falls back to the
/// whole download. On error the file is the caller's to remove.
pub(super) async fn fetch(
    want: &Want<'_>,
    path: &Path,
    seeds: &[PathBuf],
    progress: &mut impl FnMut(Progress) -> ControlFlow<()>,
) -> Result<Outcome> {
    if let Some(local) = want.url.strip_prefix("file://") {
        return copy_local(Path::new(local), want, path, progress).map(|()| Outcome::Copied);
    }
    if !seeds.is_empty() {
        match delta(want, path, seeds, progress).await {
            Ok(outcome) => return Ok(outcome),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                tracing::info!(url = want.url, error = %e, "no delta download; fetching the whole file");
                let _ = fs::remove_file(path);
            }
        }
    }
    download(want, path, progress)
        .await
        .map(|()| Outcome::Downloaded)
}

/// A `file://` release (the `box_image_url` setting pointed at a local
/// `packaging/box-image/out`): cloned, then verified like a download.
fn copy_local(
    source: &Path,
    want: &Want<'_>,
    path: &Path,
    progress: &mut impl FnMut(Progress) -> ControlFlow<()>,
) -> Result<()> {
    crate::disks::clone(source, path).map_err(Error::io(format!("copy {}", source.display())))?;
    if progress(Progress {
        done: want.size,
        total: want.size,
    })
    .is_break()
    {
        return Err(Error::Cancelled);
    }
    let len = fs::metadata(path)
        .map_err(Error::io(format!("stat {}", path.display())))?
        .len();
    if len != want.size {
        return Err(Error::Size {
            expected: want.size,
            actual: len,
        });
    }
    if !file_matches(path, Some(want.size), want.sha256)? {
        return Err(Error::Sha256);
    }
    Ok(())
}

/// Streams `want.url` to a new file at `path`, hashing as it goes, and checks
/// the size and sha256.
async fn download(
    want: &Want<'_>,
    path: &Path,
    progress: &mut impl FnMut(Progress) -> ControlFlow<()>,
) -> Result<()> {
    let url = want.url;
    let http = |e: reqwest::Error| Error::Http {
        url: url.to_owned(),
        message: e.to_string(),
    };
    let mut response = client()?
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(http)?;
    if let Some(len) = response.content_length() {
        if len != want.size {
            return Err(Error::Size {
                expected: want.size,
                actual: len,
            });
        }
    }

    let mut out = tokio::fs::File::create_new(path)
        .await
        .map_err(Error::io(format!("create {}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let total = want.size;
    if progress(Progress { done: 0, total }).is_break() {
        return Err(Error::Cancelled);
    }
    while let Some(chunk) = response.chunk().await.map_err(http)? {
        received += chunk.len() as u64;
        if received > want.size {
            return Err(Error::Size {
                expected: want.size,
                actual: received,
            });
        }
        hasher.update(&chunk);
        out.write_all(&chunk)
            .await
            .map_err(Error::io(format!("write {}", path.display())))?;
        if progress(Progress {
            done: received,
            total,
        })
        .is_break()
        {
            return Err(Error::Cancelled);
        }
    }
    if received < want.size {
        return Err(Error::Size {
            expected: want.size,
            actual: received,
        });
    }
    if format!("{:x}", hasher.finalize()) != want.sha256 {
        return Err(Error::Sha256);
    }
    out.sync_all()
        .await
        .map_err(Error::io(format!("sync {}", path.display())))
}

/// The delta path: the release's chunk list, the seeds' chunks by hash, a
/// copy of every chunk a seed has (each checked against its listed hash),
/// range requests for the rest, then the whole file against the manifest.
async fn delta(
    want: &Want<'_>,
    path: &Path,
    seeds: &[PathBuf],
    progress: &mut impl FnMut(Progress) -> ControlFlow<()>,
) -> Result<Outcome> {
    let url = want.url;
    let http = |e: reqwest::Error| Error::Http {
        url: url.to_owned(),
        message: e.to_string(),
    };
    let client = client()?;
    let list_url = chunks::list_name(url);
    let response = client.get(&list_url).send().await.map_err(http)?;
    if response.status() != StatusCode::OK {
        return Err(Error::Other(format!(
            "{list_url}: HTTP {}",
            response.status()
        )));
    }
    let bytes = read_body(response, chunks::MAX_LIST_BYTES, &list_url).await?;
    let list: ChunkList =
        serde_json::from_slice(&bytes).map_err(|e| Error::Other(format!("{list_url}: {e}")))?;
    if !list.describes(want.size, want.sha256) {
        return Err(Error::Other(format!(
            "{list_url} does not describe the file the manifest names"
        )));
    }

    // Every chunk any seed has, by hash: (seed, offset, len).
    let mut index: HashMap<String, (usize, u64, u32)> = HashMap::new();
    for (i, seed) in seeds.iter().enumerate() {
        let seed_list =
            chunks::chunk_file(seed).map_err(Error::io(format!("read {}", seed.display())))?;
        for chunk in seed_list.chunks {
            index
                .entry(chunk.sha256)
                .or_insert((i, chunk.offset, chunk.len));
        }
    }
    let total = want.size;
    report(progress, 0, total)?;

    let out = File::create_new(path).map_err(Error::io(format!("create {}", path.display())))?;
    out.set_len(total)
        .map_err(Error::io(format!("size {}", path.display())))?;
    let mut seed_files: Vec<Option<File>> = seeds.iter().map(|_| None).collect();
    let mut buf = Vec::new();
    let mut copied = 0u64;
    let mut missing: Vec<(u64, u64)> = Vec::new();
    for chunk in &list.chunks {
        let mut found = false;
        if let Some(&(i, offset, len)) = index.get(&chunk.sha256) {
            if len == chunk.len {
                if seed_files[i].is_none() {
                    seed_files[i] = Some(
                        File::open(&seeds[i])
                            .map_err(Error::io(format!("open {}", seeds[i].display())))?,
                    );
                }
                let seed = seed_files[i].as_ref().expect("opened above");
                buf.resize(len as usize, 0);
                if seed.read_exact_at(&mut buf, offset).is_ok()
                    && format!("{:x}", Sha256::digest(&buf)) == chunk.sha256
                {
                    out.write_all_at(&buf, chunk.offset)
                        .map_err(Error::io(format!("write {}", path.display())))?;
                    copied += u64::from(len);
                    found = true;
                }
            }
        }
        if !found {
            match missing.last_mut() {
                Some((offset, len)) if *offset + *len == chunk.offset => {
                    *len += u64::from(chunk.len)
                }
                _ => missing.push((chunk.offset, u64::from(chunk.len))),
            }
        }
        report(progress, copied, total)?;
    }
    drop(seed_files);
    let missing = coalesce(missing, COALESCE_GAP);

    let mut fetched = 0u64;
    for (offset, len) in missing {
        let mut response = client
            .get(url)
            .header(RANGE, format!("bytes={offset}-{}", offset + len - 1))
            .send()
            .await
            .map_err(http)?;
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(Error::Other(format!(
                "{url}: HTTP {} to a range request",
                response.status()
            )));
        }
        let mut at = offset;
        let mut got = 0u64;
        while let Some(piece) = response.chunk().await.map_err(http)? {
            got += piece.len() as u64;
            if got > len {
                return Err(Error::Other(format!(
                    "{url}: a range response ran past the {len} bytes asked for"
                )));
            }
            out.write_all_at(&piece, at)
                .map_err(Error::io(format!("write {}", path.display())))?;
            at += piece.len() as u64;
            fetched += piece.len() as u64;
            report(progress, copied + fetched, total)?;
        }
        if got != len {
            return Err(Error::Other(format!(
                "{url}: a range response gave {got} of the {len} bytes asked for"
            )));
        }
    }
    out.sync_all()
        .map_err(Error::io(format!("sync {}", path.display())))?;
    drop(out);
    // The manifest has the last word, whatever the chunk list said.
    if !file_matches(path, Some(total), want.sha256)? {
        return Err(Error::Sha256);
    }
    Ok(Outcome::Delta { copied, fetched })
}

/// Tells `progress` where a delta is (a gap fetched twice could push `done`
/// past `total`, so it is clamped); `Cancelled` when it says stop.
fn report(
    progress: &mut impl FnMut(Progress) -> ControlFlow<()>,
    done: u64,
    total: u64,
) -> Result<()> {
    if progress(Progress {
        done: done.min(total),
        total,
    })
    .is_break()
    {
        return Err(Error::Cancelled);
    }
    Ok(())
}

/// Merges ranges (sorted, non-overlapping) separated by less than `gap`.
fn coalesce(ranges: Vec<(u64, u64)>, gap: u64) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (offset, len) in ranges {
        match out.last_mut() {
            Some((o, l)) if offset <= *o + *l + gap => *l = offset + len - *o,
            _ => out.push((offset, len)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::coalesce;

    #[test]
    fn nearby_ranges_merge() {
        assert_eq!(
            coalesce(vec![(0, 10), (12, 5), (100, 1), (101, 1)], 4),
            [(0, 17), (100, 2)]
        );
        assert_eq!(coalesce(vec![(5, 5), (50, 5)], 0), [(5, 5), (50, 5)]);
        assert!(coalesce(vec![], 64).is_empty());
    }
}
