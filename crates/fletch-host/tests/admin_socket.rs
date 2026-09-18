//! The admin socket's own manners.
//!
//! Its own test binary, not a second test beside the pairing one: `boot`
//! publishes the engine's runtime handle process-wide (`host::runtime`, a
//! `OnceLock`), so two hosts booted in one process would share whichever
//! runtime got there first. One integration test file is one process, which
//! keeps each host's tasks on its own runtime.

use fletch_core::host::HeadlessRelay;
use fletch_host::{admin, serve};
use serde_json::json;

/// An unknown op is refused in the wire protocol's words rather than answered,
/// and a socket that is not there is a legible error rather than a panic —
/// the two things every client subcommand depends on.
#[tokio::test(flavor = "multi_thread")]
async fn the_admin_socket_refuses_what_it_does_not_know() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_path_buf();

    assert_eq!(
        admin::call(&data_dir, "status", json!({})).await,
        Err(admin::NOT_RUNNING.to_string()),
        "with no host running, the client says how to start one"
    );

    serve::start(serve::Config {
        data_dir: data_dir.clone(),
        port: Some(0),
        relay: HeadlessRelay::Off,
        name: None,
        handle_signals: false,
    })
    .await
    .expect("the host should boot");

    assert_eq!(
        admin::call(&data_dir, "spawn_agent", json!({})).await,
        Err(fletch_host::ops::UNKNOWN_OP.to_string()),
        "the admin socket is not the device surface"
    );
    assert!(
        admin::call(&data_dir, "revoke_device", json!({ "deviceId": "nobody" }))
            .await
            .unwrap_err()
            .contains("nobody"),
        "revoking a device that is not there says so"
    );
    assert_eq!(
        admin::call(&data_dir, "approvals_list", json!({}))
            .await
            .expect("approvals_list"),
        json!([]),
        "nothing is waiting on a host where nothing has run"
    );
}
