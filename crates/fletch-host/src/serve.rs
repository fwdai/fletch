//! Bringing the host up and keeping it up.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fletch_core::host::sink::NullSink;
use fletch_core::host::{self, BootConfig, Engine, HeadlessRelay, RemoteBoot};

use crate::admin;
use crate::ops::Admin;

/// What `fletch-host serve` was asked for. Every field is this run only:
/// nothing here is written to the database, which is why passing `--port` once
/// does not silently re-point a host that is started without it next time.
pub struct Config {
    pub data_dir: PathBuf,
    /// `None` uses the stored `remote.port`, or the protocol's default.
    pub port: Option<u16>,
    pub relay: HeadlessRelay,
    /// What a paired device calls this host. `None` asks the machine.
    pub name: Option<String>,
    /// Install the SIGINT/SIGTERM handler that kills the agents and exits. A
    /// test passes `false`: the handler is process-wide, and a test process
    /// must not exit when it is Ctrl-C'd.
    pub handle_signals: bool,
}

/// The host's data directory when `--data-dir` says nothing:
/// `~/Library/Application Support/fletch-host` on macOS,
/// `$XDG_DATA_HOME/fletch-host` on Linux, with a `dev` subfolder under a debug
/// build.
///
/// Deliberately *not* the desktop's directory (`fletch_core::data_dir`, which
/// is keyed by the app's bundle id). One machine may well run both — a Mac mini
/// serving agents that its owner also uses at the keyboard — and two engines
/// sharing one SQLite database would each sweep the other's live agents as
/// orphans. The `dev` split is the same rule applied to one binary built twice:
/// `data_dir` does it for the desktop, and a debug host testing a change must
/// not open the database a release host is serving from.
pub fn default_data_dir() -> PathBuf {
    let base = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("fletch-host");
    if cfg!(debug_assertions) {
        base.join("dev")
    } else {
        base
    }
}

/// Claim this host's roots for the two kinds of state the engine keeps outside
/// its data dir: agent checkouts (`<data-dir>/workspaces`) and RPC mailboxes
/// (`<data-dir>/rpc`).
///
/// The engine already splits those two roots per *build*
/// (`fletch_core::build_state_subpath`: `dev/` under debug) and the reason is
/// exclusivity, not tidiness. Agent ids are drawn from a ~300-entry pool of
/// place names and recycled after archive, with the database as the only
/// authority — so two engines with two databases on one filesystem will
/// eventually hand out the same name, land two live agents in one checkout
/// directory (which provision clears, believing it can only be its own
/// crash-orphan), and read each other's live mailboxes as orphans to sweep.
///
/// A host beside a desktop is exactly two such engines — and so is a host
/// beside another host. `--data-dir` is what separates two hosts (its database,
/// its socket, its host key), so these roots hang off it too: without that,
/// `--data-dir` gives an operator two databases sharing one agent namespace and
/// one set of mailboxes, which is the collision above with the port number as
/// its only fig leaf. The per-build split still applies underneath, so a debug
/// and a release host pointed at *one* data dir stay separate as well.
///
/// The data dir is fine to hang an agent's writable tree off: the one policy
/// that carves a data dir out of what a sandboxed agent may touch names the
/// desktop's bundle dir specifically (`sandbox::seatbelt`'s `deny_app_data_dir`
/// / `fletch_core::BUNDLE_ID`), and a container sees nothing but the paths bound
/// into it.
///
/// Anything already set — an operator's own layout, a disk with room for
/// checkouts — is left alone, and then uniqueness across hosts is theirs to
/// keep. Called before the engine boots, and before anything else in the
/// process could be reading the environment.
fn claim_state_roots(data_dir: &Path) {
    // Absolute: a `--data-dir` may be relative, but a seatbelt profile and a
    // container bind mount are both built from these paths and neither takes a
    // relative one. `absolute` is pure — no existence check, no symlink
    // resolution — so it is safe here, before the dir is created.
    let base = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    for key in [
        fletch_core::workspace::WORKSPACES_ROOT_ENV,
        fletch_core::rpc::RPC_ROOT_ENV,
    ] {
        if std::env::var_os(key).filter(|v| !v.is_empty()).is_none() {
            std::env::set_var(key, &base);
        }
    }
}

/// Boot the engine and start answering on the admin socket.
///
/// The half `serve` shares with the end-to-end test: everything except waiting
/// forever. The listener is bound before the accept loop is spawned, so once
/// this returns a client can connect without racing.
pub async fn start(config: Config) -> Result<Arc<Admin>, String> {
    let Config {
        data_dir,
        port,
        relay,
        name,
        handle_signals,
    } = config;
    claim_state_roots(&data_dir);
    prepare_data_dir(&data_dir)?;
    let listener = admin::bind(&data_dir).await?;
    let socket = admin::socket_path(&data_dir);

    let engine = match host::boot(BootConfig {
        data_dir: data_dir.clone(),
        // `claim_state_roots` above pointed the engine at roots this host
        // derived for itself, so they are this host's to sweep — the engine
        // cannot tell that from how they were configured (see `StateRoots`).
        state_roots: host::StateRoots::Owned,
        // Nowhere, on purpose. `boot` wraps this in a fanout with its own
        // broadcast, and the remote taps subscribe to *that*, so events still
        // reach every paired device — there is simply no second destination
        // (the desktop's is its webview).
        sink: Arc::new(NullSink),
        // Nobody is looking at this host: there is no window to look at. Push
        // alerts are never suppressed as a result, which is the answer a host
        // wants.
        focus: Box::new(|| false),
        runtime: tokio::runtime::Handle::current(),
        remote: RemoteBoot::Headless { port, relay, name },
        // No microphone, no window, no whisper build: the five `dictation_*`
        // ops answer "unavailable" (`SupervisorDispatch::with_dictation`).
        dictation: None,
        // Nobody to prompt. A database that will not open is an exit code and a
        // log line, not a dialog.
        recover_db: None,
        on_supervisor: None,
        signals: handle_signals.then(|| -> host::boot::ExitHook {
            Box::new(move || {
                // The children are already dead (`boot` kills them before
                // calling this). Take the socket with us so the next start does
                // not have to reason about a stale one, then leave with a clean
                // status: a service manager must not read an orderly stop as a
                // crash.
                let _ = std::fs::remove_file(&socket);
                tracing::info!("fletch-host stopped");
                std::process::exit(0);
            })
        }),
    }) {
        Ok(engine) => engine,
        Err(e) => {
            // The socket was bound before the engine came up (so that a client
            // cannot beat it there); a host that never started must not leave it
            // behind for the next one to reason about.
            let _ = std::fs::remove_file(admin::socket_path(&data_dir));
            return Err(e.to_string());
        }
    };

    log_where_we_are(&engine, &data_dir);
    let admin = Arc::new(Admin::new(engine, data_dir));
    tokio::spawn(admin::serve(admin.clone(), listener));
    Ok(admin)
}

/// Boot the host and serve until a signal ends the process.
pub async fn run(config: Config) -> Result<(), String> {
    let _admin = start(config).await?;
    // Nothing left for this task to do: the accept loop and the engine's own
    // tasks are running, and the signal handler `boot` installed is what ends
    // the process.
    std::future::pending::<()>().await;
    Ok(())
}

/// Create the data dir if it is new and make sure it is the service user's
/// alone. Applied on every start, not only on creation: a directory that was
/// widened by hand (or by a restore) is a host whose database, host key and
/// device store are readable by every account on the machine.
fn prepare_data_dir(data_dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(data_dir)
        .map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
    let mode = std::fs::metadata(data_dir)
        .map_err(|e| format!("cannot read {}: {e}", data_dir.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(admin::DIR_MODE))
            .map_err(|e| format!("cannot restrict {} to this user: {e}", data_dir.display()))?;
    }
    Ok(())
}

/// The one thing an operator needs from the log to get a phone paired: where
/// this host is listening and who it says it is.
fn log_where_we_are(engine: &Engine, data_dir: &Path) {
    let Some(remote) = engine.remote.as_ref() else {
        return;
    };
    let status = remote.status();
    tracing::info!(
        port = status.port,
        host_id = %status.host_id,
        addresses = %status.addresses.join(", "),
        data_dir = %data_dir.display(),
        "fletch-host serving"
    );
    if let Some(error) = status.error {
        tracing::error!(error = %error, "the remote directory is not usable; pairing will fail");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two engines on one machine must not share a byte of state. The default
    /// data dir is neither the desktop's nor a release host's, and the checkout
    /// and mailbox roots hang off *this* host's data dir — so two hosts started
    /// with two `--data-dir`s share no agent namespace either. The rest of the
    /// separation (the per-build `dev/` segment under those roots) is the
    /// engine's and is tested there.
    #[test]
    fn the_host_keeps_its_state_to_itself() {
        let default_dir = default_data_dir();
        assert!(!default_dir.starts_with(fletch_core::data_dir()));
        assert!(default_dir.to_string_lossy().contains("fletch-host"));
        // Tests build in debug, so this run's dir is the debug one.
        assert_eq!(default_dir.file_name().unwrap(), "dev");

        // Process-global, and this is the only test that touches it — the same
        // contract the engine's override tests keep.
        let td = tempfile::tempdir().unwrap();
        let data_dir = td.path().join("host-a");
        claim_state_roots(&data_dir);
        let checkouts = fletch_core::workspace::checkouts_root().expect("a checkouts root");
        let mailboxes = fletch_core::rpc::mailbox_dir("orkney").expect("a mailbox dir");
        for path in [&checkouts, &mailboxes] {
            assert!(
                path.starts_with(&data_dir),
                "{} is outside this host's data dir, so --data-dir does not isolate it",
                path.display(),
            );
        }
        assert!(checkouts.ends_with("dev/workspaces"), "{checkouts:?}");
        assert!(mailboxes.ends_with("dev/rpc/orkney"), "{mailboxes:?}");
    }
}
