//! Content-defined chunking of image files, for delta downloads.
//!
//! A release carries, next to each image file, a `<file>.chunks.json` listing
//! the file's chunks (offset, length, sha256) under a fixed algorithm. A host
//! that has an older version of the same image chunks its copy with the same
//! algorithm, copies every chunk whose hash the new list also has, and fetches
//! only the rest with HTTP range requests ([`super::fetch`]). Cut points depend
//! on content, not position, so an insertion early in the file shifts nothing
//! that follows out of alignment.
//!
//! The algorithm is a gear hash: `h = (h << 1) + GEAR[byte]` over the bytes of
//! the current chunk, a cut where the low bits of `h` are zero, with a minimum
//! and a maximum chunk length. The gear table is the first 8 bytes of
//! `sha256([b])` for each byte value, so any implementation can reproduce it
//! without sharing a table. A mismatch between producer and consumer costs
//! efficiency only: the assembled file is still verified by its sha256.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Names the cut rule and the table; bump it if either changes. The producer
/// is `fletch-box image chunk` (the Box image workflow), the consumer
/// [`super::fetch`]; both are this module.
pub const ALGORITHM: &str = "gear-sha256-v1";
/// Average chunk length. 256 KiB reused 91% of a rebuilt 160 MB image with
/// one new file and two edits, against 82% at 1 MiB.
pub const AVG: u64 = 256 * 1024;
pub const MIN: u64 = AVG / 4;
pub const MAX: u64 = AVG * 4;
const MASK: u64 = AVG - 1;
/// The chunk lists are a few hundred KB at most; anything past this is not one.
pub const MAX_LIST_BYTES: u64 = 8 << 20;

fn gear() -> &'static [u64; 256] {
    static GEAR: OnceLock<[u64; 256]> = OnceLock::new();
    GEAR.get_or_init(|| {
        let mut table = [0u64; 256];
        for (b, slot) in table.iter_mut().enumerate() {
            let digest = Sha256::digest([b as u8]);
            *slot = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"));
        }
        table
    })
}

/// One chunk of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub offset: u64,
    pub len: u32,
    /// Lowercase hex.
    pub sha256: String,
}

/// `<file>.chunks.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkList {
    pub algorithm: String,
    pub avg: u64,
    /// The whole file.
    pub size: u64,
    pub sha256: String,
    pub chunks: Vec<Chunk>,
}

impl ChunkList {
    /// Whether this list was made by this algorithm and describes a file of
    /// `size` bytes with `sha256`, as a contiguous run of chunks from 0.
    pub fn describes(&self, size: u64, sha256: &str) -> bool {
        if self.algorithm != ALGORITHM
            || self.avg != AVG
            || self.size != size
            || !self.sha256.eq_ignore_ascii_case(sha256)
        {
            return false;
        }
        let mut next = 0u64;
        for chunk in &self.chunks {
            if chunk.offset != next || chunk.len == 0 || !super::well_formed_sha256(&chunk.sha256) {
                return false;
            }
            next += u64::from(chunk.len);
        }
        next == size
    }
}

/// The name of a file's chunk list in a release.
pub fn list_name(file: &str) -> String {
    format!("{file}.chunks.json")
}

/// Streams bytes in and cuts chunks; `finish` yields the list.
pub struct Chunker {
    hash: u64,
    /// Bytes in the current chunk so far.
    len: u64,
    start: u64,
    hasher: Sha256,
    chunks: Vec<Chunk>,
}

impl Default for Chunker {
    fn default() -> Self {
        Self::new()
    }
}

impl Chunker {
    pub fn new() -> Chunker {
        Chunker {
            hash: 0,
            len: 0,
            start: 0,
            hasher: Sha256::new(),
            chunks: Vec::new(),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        let gear = gear();
        let mut from = 0usize;
        for (i, &byte) in data.iter().enumerate() {
            if self.len >= MIN {
                self.hash = (self.hash << 1).wrapping_add(gear[byte as usize]);
            }
            self.len += 1;
            if (self.len > MIN && self.hash & MASK == 0) || self.len >= MAX {
                self.hasher.update(&data[from..=i]);
                from = i + 1;
                self.cut();
            }
        }
        if from < data.len() {
            self.hasher.update(&data[from..]);
        }
    }

    fn cut(&mut self) {
        let hasher = std::mem::replace(&mut self.hasher, Sha256::new());
        self.chunks.push(Chunk {
            offset: self.start,
            len: u32::try_from(self.len).expect("a chunk is at most MAX bytes"),
            sha256: format!("{:x}", hasher.finalize()),
        });
        self.start += self.len;
        self.len = 0;
        self.hash = 0;
    }

    pub fn finish(mut self) -> Vec<Chunk> {
        if self.len > 0 {
            self.cut();
        }
        self.chunks
    }
}

/// The chunks of the file at `path`, with its size and whole-file sha256.
pub fn chunk_file(path: &Path) -> io::Result<ChunkList> {
    let mut file = File::open(path)?;
    let mut chunker = Chunker::new();
    let mut whole = Sha256::new();
    let mut size = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        chunker.update(&buf[..n]);
        whole.update(&buf[..n]);
        size += n as u64;
    }
    Ok(ChunkList {
        algorithm: ALGORITHM.to_owned(),
        avg: AVG,
        size,
        sha256: format!("{:x}", whole.finalize()),
        chunks: chunker.finish(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random bytes (xorshift), so the tests need no files.
    fn noise(seed: u64, len: usize) -> Vec<u8> {
        let mut x = seed | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    fn chunks_of(data: &[u8]) -> Vec<Chunk> {
        let mut c = Chunker::new();
        c.update(data);
        c.finish()
    }

    #[test]
    fn the_gear_table_is_fixed() {
        // sha256(0x00) starts 6e340b9c..., sha256(0xff) starts a8100ae6...
        assert_eq!(gear()[0], 0x6e34_0b9c_ffb3_7a98);
        assert_eq!(gear()[255], 0xa810_0ae6_aa19_40d0);
    }

    #[test]
    fn chunks_tile_the_input_within_the_bounds() {
        let data = noise(7, 6 << 20);
        let chunks = chunks_of(&data);
        let mut next = 0u64;
        for chunk in &chunks {
            assert_eq!(chunk.offset, next);
            assert!(u64::from(chunk.len) <= MAX);
            next += u64::from(chunk.len);
        }
        assert_eq!(next, data.len() as u64);
        // Every chunk but the last reached the minimum.
        assert!(chunks[..chunks.len() - 1]
            .iter()
            .all(|c| u64::from(c.len) > MIN));
        let avg = data.len() as u64 / chunks.len() as u64;
        assert!((AVG / 3..AVG * 3).contains(&avg), "average chunk {avg}");
        // Each chunk's hash is the hash of its bytes.
        let c = &chunks[1];
        let (o, l) = (c.offset as usize, c.len as usize);
        assert_eq!(c.sha256, format!("{:x}", Sha256::digest(&data[o..o + l])));
    }

    #[test]
    fn feeding_in_pieces_gives_the_same_chunks() {
        let data = noise(11, 3 << 20);
        let whole = chunks_of(&data);
        let mut c = Chunker::new();
        for piece in data.chunks(7919) {
            c.update(piece);
        }
        assert_eq!(c.finish(), whole);
    }

    #[test]
    fn an_insertion_shifts_only_nearby_chunks() {
        let old = noise(3, 8 << 20);
        let mut new = old[..2 << 20].to_vec();
        new.extend_from_slice(&noise(99, 300_000));
        new.extend_from_slice(&old[2 << 20..]);
        let had: std::collections::HashSet<_> =
            chunks_of(&old).into_iter().map(|c| c.sha256).collect();
        let new_chunks = chunks_of(&new);
        let reused: u64 = new_chunks
            .iter()
            .filter(|c| had.contains(&c.sha256))
            .map(|c| u64::from(c.len))
            .sum();
        assert!(
            reused * 100 / new.len() as u64 >= 85,
            "reused {reused} of {}",
            new.len()
        );
    }

    #[test]
    fn a_list_describes_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("img");
        let data = noise(5, 1 << 20);
        std::fs::write(&path, &data).unwrap();
        let list = chunk_file(&path).unwrap();
        assert_eq!(list.chunks, chunks_of(&data));
        assert_eq!(list.size, data.len() as u64);
        assert!(list.describes(data.len() as u64, &list.sha256.to_uppercase()));
        assert!(!list.describes(data.len() as u64 + 1, &list.sha256));
        let mut broken = list.clone();
        broken.chunks[0].len += 1;
        assert!(!broken.describes(data.len() as u64, &list.sha256));
        assert_eq!(list_name("base.erofs"), "base.erofs.chunks.json");
    }
}
