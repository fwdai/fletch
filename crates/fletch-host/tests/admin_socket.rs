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

    // The engine facts an operator with no window checks first. On a host that
    // has just booted with an empty data dir all three are known: no agents, a
    // selected sandbox engine (whatever `serve` chose for this platform), and no
    // GitHub token.
    let status = admin::call(&data_dir, "status", json!({}))
        .await
        .expect("status");
    assert_eq!(
        status["agents"],
        json!({ "total": 0, "running": 0 }),
        "{status}"
    );
    assert!(
        ["sandbox-exec", "docker", "podman"]
            .contains(&status["sandboxEngine"].as_str().unwrap_or_default()),
        "sandboxEngine names one of the engines: {status}"
    );
    assert_eq!(
        status["githubConnected"],
        json!(false),
        "nothing has signed this host in"
    );
    assert!(
        status["providers"]["signedIn"].is_array() && status["providers"]["signedOut"].is_array(),
        "status says which provider CLIs are signed in: {status}"
    );

    // The provider surface. Nothing is asserted about what is installed on the
    // machine running this — CI has none of these CLIs — only that every known
    // provider is answered for, with the fields the CLI table prints.
    let providers = admin::call(&data_dir, "provider_status", json!({}))
        .await
        .expect("provider_status");
    let providers = providers.as_array().cloned().unwrap_or_default();
    let ids: Vec<&str> = providers.iter().filter_map(|p| p["id"].as_str()).collect();
    for known in ["claude", "codex", "cursor", "antigravity", "opencode", "pi"] {
        assert!(ids.contains(&known), "{known} is missing from {ids:?}");
    }
    let claude = providers
        .iter()
        .find(|p| p["id"] == json!("claude"))
        .expect("claude is probed");
    assert_eq!(claude["label"], json!("Claude Code"), "{claude}");
    assert_eq!(
        claude["loginCommand"],
        json!("claude auth login"),
        "the row carries the vendor's own sign-in command: {claude}"
    );
    assert!(claude["installed"].is_boolean(), "{claude}");
    if claude["installed"] == json!(false) {
        assert_eq!(
            claude["auth"],
            json!(null),
            "a CLI that is not here has no login state: {claude}"
        );
    }

    // What `provider login` refuses, and why.
    assert_eq!(
        admin::call(
            &data_dir,
            "provider_login_command",
            json!({ "id": "nonesuch" })
        )
        .await,
        Err("unknown provider nonesuch".to_string()),
        "a provider nobody has heard of is named in the error"
    );
    let out_of_band = admin::call(
        &data_dir,
        "provider_login_command",
        json!({ "id": "antigravity" }),
    )
    .await
    .unwrap_err();
    assert!(
        out_of_band.contains("Antigravity") && out_of_band.contains("out of band"),
        "a provider with no login command says so: {out_of_band}"
    );

    a_wedged_cli_cannot_hang_either_call(&data_dir, dir.path()).await;
}

/// A CLI that never answers `--version` — a broken install, or a custom binary
/// path pointed at the wrong thing. `status` must not run one at all (it is
/// what a liveness check calls), and `provider_status`, which does want the
/// version, must give up on it and still answer.
async fn a_wedged_cli_cannot_hang_either_call(data_dir: &std::path::Path, tmp: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let ran = tmp.join("version-probe-ran");
    let wedged = tmp.join("wedged-claude");
    std::fs::write(
        &wedged,
        format!("#!/bin/sh\necho ran >> '{}'\nsleep 300\n", ran.display()),
    )
    .unwrap();
    std::fs::set_permissions(&wedged, std::fs::Permissions::from_mode(0o755)).unwrap();
    // The same door a user's custom binary path goes through.
    fletch_core::bin_resolve::set_agent_overrides(std::collections::HashMap::from([(
        "claude".to_string(),
        wedged.to_string_lossy().into_owned(),
    )]));

    let status = admin::call(data_dir, "status", json!({}))
        .await
        .expect("status");
    assert!(
        !ran.exists(),
        "status answered by running a provider CLI: {status}"
    );

    let started = std::time::Instant::now();
    let providers = admin::call(data_dir, "provider_status", json!({}))
        .await
        .expect("provider_status");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "provider_status waited on a CLI that sleeps for 300s"
    );
    assert!(ran.exists(), "provider_status did probe the version");
    let claude = providers
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == json!("claude")))
        .expect("claude is probed")
        .clone();
    assert_eq!(claude["installed"], json!(true), "{claude}");
    assert_eq!(
        claude["version"],
        json!(null),
        "a probe that timed out reports no version: {claude}"
    );

    fletch_core::bin_resolve::set_agent_overrides(std::collections::HashMap::new());
}
