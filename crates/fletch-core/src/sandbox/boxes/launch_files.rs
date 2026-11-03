//! A launch's files in its box's guest, under `/data/run`: its env file,
//! written once before the launch's first process (`BoxEngine::deliver_env`),
//! and one args file per process, written just before that process starts
//! ([`LaunchFiles::stage`]), which `fletch-guest exec --args-file` reads and
//! removes.
//!
//! The host writes every one of them, never the launcher, and each write
//! holds the launch's lock, which its teardown takes too
//! ([`LaunchFiles::close`]). So a write either finished before the teardown
//! removed the launch's files, or never happens: no write can land after the
//! removal, however a session is killed and whatever its launcher's processes
//! were doing.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::error::{Error, Result};

/// `/data/run/<launch id>`, how the names of a launch's files there start:
/// its env file ([`env_file`]) and the args file of each of its processes
/// (`<stem>.<n>.args`).
pub(super) fn stem(launch_id: &str) -> PathBuf {
    PathBuf::from(fletch_box::guest::ENV_FILE_DIR).join(launch_id)
}

/// `/data/run/<launch id>.env`.
pub(super) fn env_file(launch_id: &str) -> PathBuf {
    PathBuf::from(fletch_box::guest::ENV_FILE_DIR).join(format!("{launch_id}.env"))
}

/// Removes a launch's files in the guest, `$1` their [`stem`]: the env file
/// and every args file no `fletch-guest exec` took. A launch id is a UUID,
/// all of one length, so no other launch's files share the pattern.
pub(super) const REMOVE: &str = "rm -f -- \"$1.env\" \"$1\".*.args";

/// Each argument followed by a NUL, the one byte no argument can hold: what
/// `fletch-guest exec --args-file` reads.
pub(super) fn encode_args(args: &[String]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(args.iter().map(|a| a.len() + 1).sum());
    for arg in args {
        bytes.extend_from_slice(arg.as_bytes());
        bytes.push(0);
    }
    bytes
}

/// One launch's files, and the lock every write of them and their removal
/// take.
pub(super) struct LaunchFiles {
    launch_id: String,
    state: Mutex<State>,
}

struct State {
    /// False once the launch was torn down: nothing more is written.
    open: bool,
    /// Args files written so far, which names the next.
    staged: u64,
}

impl LaunchFiles {
    pub(super) fn new(launch_id: &str) -> Self {
        Self {
            launch_id: launch_id.to_string(),
            state: Mutex::new(State {
                open: true,
                staged: 0,
            }),
        }
    }

    /// Write `contents` as the launch's env file with `write` (the guest
    /// path, then the bytes), returning its path; refused once the launch was
    /// torn down.
    pub(super) fn write_env(
        &self,
        contents: &[u8],
        write: impl FnOnce(&Path, &[u8]) -> Result<()>,
    ) -> Result<PathBuf> {
        let state = self.state.lock();
        self.refuse_when_closed(&state)?;
        let file = env_file(&self.launch_id);
        write(&file, contents)?;
        Ok(file)
    }

    /// Write `args` as the args file of the launch's next process with
    /// `write`, returning its path, a name of that process's own; refused
    /// once the launch was torn down. The lock is held across the write, so
    /// a teardown that comes meanwhile waits for it.
    pub(super) fn stage(
        &self,
        args: &[String],
        write: impl FnOnce(&Path, &[u8]) -> Result<()>,
    ) -> Result<PathBuf> {
        let mut state = self.state.lock();
        self.refuse_when_closed(&state)?;
        state.staged += 1;
        let file = PathBuf::from(format!(
            "{}.{}.args",
            stem(&self.launch_id).display(),
            state.staged
        ));
        write(&file, &encode_args(args))?;
        Ok(file)
    }

    /// End the launch's writes, waiting for one under way, then `remove` its
    /// files ([`REMOVE`], given the [`stem`]). Every write either finished
    /// before this or is refused after it.
    pub(super) fn close(&self, remove: impl FnOnce(&Path)) {
        let mut state = self.state.lock();
        state.open = false;
        remove(&stem(&self.launch_id));
    }

    fn refuse_when_closed(&self, state: &State) -> Result<()> {
        if state.open {
            Ok(())
        } else {
            Err(Error::Other(format!(
                "box launch {} was torn down; nothing more is written for it",
                self.launch_id
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{mpsc, Arc};

    use super::*;

    const LAUNCH: &str = "8a0c4f2e-1d3b-4c5a-9e7f-0123456789ab";

    /// What the writes were given: each path, with its bytes.
    type Written = Arc<Mutex<Vec<(PathBuf, Vec<u8>)>>>;

    /// A write that records what it was given.
    fn recording(log: &Written) -> impl Fn(&Path, &[u8]) -> Result<()> {
        let log = log.clone();
        move |path, bytes| {
            log.lock().push((path.to_path_buf(), bytes.to_vec()));
            Ok(())
        }
    }

    #[test]
    fn each_process_gets_an_args_file_of_its_own() {
        let files = LaunchFiles::new(LAUNCH);
        let log = Arc::new(Mutex::new(Vec::new()));
        let args = [
            "--append-system-prompt".to_string(),
            "it's\n\"x\"".into(),
            String::new(),
        ];
        let first = files.stage(&args, recording(&log)).unwrap();
        let second = files.stage(&[], recording(&log)).unwrap();
        assert_eq!(first, Path::new(&format!("/data/run/{LAUNCH}.1.args")));
        assert_eq!(second, Path::new(&format!("/data/run/{LAUNCH}.2.args")));
        let log = log.lock();
        assert_eq!(log[0].1, b"--append-system-prompt\0it's\n\"x\"\0\0");
        assert!(log[1].1.is_empty(), "no arguments is an empty file");
        let env = files.write_env(b"A=1\n", |_, _| Ok(())).unwrap();
        assert_eq!(env, Path::new(&format!("/data/run/{LAUNCH}.env")));
    }

    /// Once torn down, the launch writes nothing: the write is never called.
    #[test]
    fn nothing_is_written_after_the_teardown() {
        let files = LaunchFiles::new(LAUNCH);
        let removed = Arc::new(Mutex::new(Vec::new()));
        let r = removed.clone();
        files.close(move |stem| r.lock().push(stem.to_path_buf()));
        assert_eq!(*removed.lock(), [stem(LAUNCH)]);

        let log = Arc::new(Mutex::new(Vec::new()));
        let err = files.stage(&["x".into()], recording(&log)).unwrap_err();
        assert!(err.to_string().contains("torn down"), "{err}");
        assert!(files.write_env(b"A=1\n", recording(&log)).is_err());
        assert!(log.lock().is_empty());
    }

    /// A teardown that comes while a write is under way waits for it, and
    /// removes the files only after it: the write cannot land afterwards.
    #[test]
    fn the_teardown_waits_for_a_write_under_way() {
        let files = Arc::new(LaunchFiles::new(LAUNCH));
        let events = Arc::new(Mutex::new(Vec::new()));
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel::<()>();

        let writer = {
            let (files, events) = (files.clone(), events.clone());
            std::thread::spawn(move || {
                files.stage(&["x".into()], |_, _| {
                    started_tx.send(()).unwrap();
                    finish_rx.recv().unwrap();
                    events.lock().push("written");
                    Ok(())
                })
            })
        };
        started_rx.recv().unwrap();
        let closer = {
            let (files, events) = (files.clone(), events.clone());
            std::thread::spawn(move || files.close(|_| events.lock().push("removed")))
        };
        // The teardown is blocked on the write for as long as it runs.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            events.lock().is_empty(),
            "removed while the write was under way"
        );
        finish_tx.send(()).unwrap();
        writer.join().unwrap().unwrap();
        closer.join().unwrap();
        assert_eq!(*events.lock(), ["written", "removed"]);
    }

    /// The removal takes the launch's env file and every args file left
    /// behind, and nothing of another launch's; with nothing left, it is no
    /// error.
    #[test]
    fn the_removal_takes_the_launchs_files_and_no_others() {
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join(LAUNCH);
        let other = dir.path().join("1b2c3d4e-5f60-4718-8a9b-abcdef012345");
        let named = |stem: &Path, suffixes: &[&str]| -> Vec<PathBuf> {
            suffixes
                .iter()
                .map(|s| PathBuf::from(format!("{}{s}", stem.display())))
                .collect()
        };
        let mine_files = named(&mine, &[".env", ".1.args", ".12.args"]);
        let their_files = named(&other, &[".env", ".1.args"]);
        for file in mine_files.iter().chain(&their_files) {
            std::fs::write(file, b"").unwrap();
        }
        let remove = || {
            std::process::Command::new("/bin/sh")
                .args(["-c", REMOVE, "sh"])
                .arg(&mine)
                .status()
                .unwrap()
        };
        assert!(remove().success());
        assert!(mine_files.iter().all(|f| !f.exists()), "{mine_files:?}");
        assert!(their_files.iter().all(|f| f.exists()), "{their_files:?}");
        assert!(remove().success());
    }
}
