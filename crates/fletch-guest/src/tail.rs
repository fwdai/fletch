//! `tail <dir> [--offsets <json>]`: follows every `*.jsonl` under `dir`,
//! recursively, and emits new bytes as JSON lines (spec §7.3, transcript
//! mirror). On reconnect the host passes `{"<path>": <bytes it holds>}` so the
//! stream resumes exactly where its copy ends; a file that shrank is announced
//! again with `create` and resent from zero.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use base64::Engine;

use crate::protocol::{write_line, Event};
use crate::Result;

const POLL: Duration = Duration::from_millis(100);

pub fn run(args: &[String]) -> Result<()> {
    let root = PathBuf::from(args.first().ok_or("tail: missing <dir>")?);
    let mut offsets = BTreeMap::new();
    let mut it = args[1..].iter();
    while let Some(opt) = it.next() {
        match opt.as_str() {
            "--offsets" => {
                let json = it.next().ok_or("tail: --offsets needs a value")?;
                offsets =
                    serde_json::from_str(json).map_err(|e| format!("tail: --offsets: {e}"))?;
            }
            other => return Err(format!("tail: unknown option {other}").into()),
        }
    }

    let mut tailer = Tailer::new(root, offsets);
    let mut out = io::stdout().lock();
    loop {
        tailer.poll(&mut out)?;
        thread::sleep(POLL);
    }
}

pub struct Tailer {
    root: PathBuf,
    /// Bytes already emitted (or already held by the host) per file.
    offsets: BTreeMap<String, u64>,
}

impl Tailer {
    pub fn new(root: PathBuf, offsets: BTreeMap<String, u64>) -> Self {
        Tailer { root, offsets }
    }

    /// One pass over the tree: announces new files, emits new bytes.
    pub fn poll(&mut self, out: &mut impl Write) -> io::Result<()> {
        let mut files = Vec::new();
        collect_jsonl(&self.root, &mut files);
        for path in files {
            let Ok(len) = fs::metadata(&path).map(|m| m.len()) else {
                continue;
            };
            let key = path.to_string_lossy().into_owned();
            let offset = match self.offsets.get(&key) {
                Some(&known) if known <= len => known,
                _ => {
                    write_line(out, &Event::Create { path: key.clone() })?;
                    0
                }
            };
            if len > offset {
                let data = read_range(&path, offset, len - offset)?;
                let data = base64::engine::general_purpose::STANDARD.encode(data);
                write_line(
                    out,
                    &Event::Append {
                        path: key.clone(),
                        data,
                    },
                )?;
            }
            self.offsets.insert(key, len);
        }
        Ok(())
    }
}

fn collect_jsonl(dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, into);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            into.push(path);
        }
    }
}

/// Reads exactly `[offset, offset + len)`, so a file growing mid-read still
/// yields a prefix whose end matches the recorded offset.
fn read_range(path: &Path, offset: u64, len: u64) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut data = Vec::with_capacity(len as usize);
    file.take(len).read_to_end(&mut data)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(tailer: &mut Tailer) -> Vec<Event> {
        let mut out = Vec::new();
        tailer.poll(&mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn new_file_is_created_then_appended_incrementally() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("projects/slug");
        fs::create_dir_all(&sub).unwrap();
        let file = sub.join("t.jsonl");
        fs::write(&file, "one\n").unwrap();
        fs::write(sub.join("ignored.json"), "{}").unwrap();
        let path = file.to_string_lossy().into_owned();

        let mut tailer = Tailer::new(dir.path().to_path_buf(), BTreeMap::new());
        assert_eq!(
            events(&mut tailer),
            vec![
                Event::Create { path: path.clone() },
                Event::Append {
                    path: path.clone(),
                    data: b64("one\n")
                },
            ]
        );
        assert!(events(&mut tailer).is_empty());

        let mut f = fs::OpenOptions::new().append(true).open(&file).unwrap();
        f.write_all(b"two\n").unwrap();
        assert_eq!(
            events(&mut tailer),
            vec![Event::Append {
                path,
                data: b64("two\n")
            }]
        );
    }

    #[test]
    fn host_offsets_resume_without_a_create() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.jsonl");
        fs::write(&file, "one\ntwo\n").unwrap();
        let path = file.to_string_lossy().into_owned();

        let offsets = BTreeMap::from([(path.clone(), 4u64)]);
        let mut tailer = Tailer::new(dir.path().to_path_buf(), offsets);
        assert_eq!(
            events(&mut tailer),
            vec![Event::Append {
                path,
                data: b64("two\n")
            }]
        );
    }

    #[test]
    fn offset_equal_to_length_emits_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.jsonl");
        fs::write(&file, "one\n").unwrap();
        let offsets = BTreeMap::from([(file.to_string_lossy().into_owned(), 4u64)]);
        let mut tailer = Tailer::new(dir.path().to_path_buf(), offsets);
        assert!(events(&mut tailer).is_empty());
    }

    #[test]
    fn a_shrunk_file_starts_over_with_create() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.jsonl");
        fs::write(&file, "one\ntwo\n").unwrap();
        let path = file.to_string_lossy().into_owned();
        let mut tailer = Tailer::new(dir.path().to_path_buf(), BTreeMap::new());
        events(&mut tailer);

        fs::write(&file, "x\n").unwrap();
        assert_eq!(
            events(&mut tailer),
            vec![
                Event::Create { path: path.clone() },
                Event::Append {
                    path,
                    data: b64("x\n")
                },
            ]
        );
    }
}
