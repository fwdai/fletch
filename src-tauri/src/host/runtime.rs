//! Where the engine's background tasks run.
//!
//! Engine code used to call `tauri::async_runtime::spawn`. That function is a
//! thin wrapper over a *global* tokio handle (`tauri::async_runtime::spawn` →
//! `RuntimeHandle::spawn` → `handle.enter()` + `tokio::spawn`), which is why it
//! works from anywhere: the Tauri `setup` closure on the main thread, and the
//! plain `std::thread`s that read agent stdout and reap child processes
//! (`child_io::spawn_reaper`, `pty_session`). A bare `tokio::spawn` would panic
//! on all of those, because there is no runtime in the thread-local context.
//!
//! So the engine keeps the shape and drops the Tauri dependency: the host hands
//! over the runtime its tasks belong to once at boot, and [`spawn`] uses that
//! handle from whatever thread the caller happens to be on. The desktop passes
//! Tauri's own runtime handle, so this is the same runtime, the same threads and
//! the same ordering as before.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

static HANDLE: OnceLock<Handle> = OnceLock::new();

/// Publish the runtime the engine spawns onto. The host calls this once, before
/// it boots anything that can spawn; a second call keeps the first, since tasks
/// already running belong to it.
pub fn init(handle: Handle) {
    if HANDLE.set(handle).is_err() {
        tracing::warn!("engine runtime: handle already set");
    }
}

/// Spawn one of the engine's background tasks.
///
/// Callable from any thread, like the `tauri::async_runtime::spawn` it replaces.
/// Before [`init`] — i.e. in unit tests, which boot no host — it falls back to
/// the caller's own runtime, so a `#[tokio::test]` works unchanged and a test
/// with no runtime at all fails with tokio's own "must be called from the
/// context of a Tokio runtime" panic rather than silently starting a second one.
#[track_caller]
pub fn spawn<F>(task: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match HANDLE.get() {
        Some(handle) => handle.spawn(task),
        None => tokio::spawn(task),
    }
}

#[cfg(test)]
mod tests {
    /// The property the engine relies on: a task spawned from a thread that has
    /// no runtime of its own still runs. This is what `tokio::spawn` cannot do
    /// and what the agent reader/reaper threads need.
    #[test]
    fn a_thread_with_no_runtime_can_still_spawn() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let handle = runtime.handle().clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // Not `super::spawn`: the process-wide handle belongs to whichever
            // test ran first, so this asserts on the mechanism, not the static.
            handle.spawn(async move {
                let _ = tx.send("ran");
            });
        })
        .join()
        .unwrap();
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            "ran"
        );
    }
}
