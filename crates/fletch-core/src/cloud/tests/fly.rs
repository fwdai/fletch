//! The Fly.io adapter against a stand-in Machines API: the exact requests each
//! operation sends, and what it makes of each answer Fly can give.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::cloud::stand_in::{Reply, Request, StandIn};
use crate::cloud::ProviderId;

const TOKEN: &str = "fo1_plain_test_token_0123456789";
const FLYV1: &str = "FlyV1 fm2_macaroonA0123456789,fm2_macaroonB9876543210";
const APP: &str = "fletch-box-ladakh-otter-k3j9x2ab";

fn client(server: &StandIn, token: &str, region: Option<&str>) -> Client {
    let connection = Connection::new(ProviderId::Fly, token, Some("acme"))
        .unwrap()
        .with_region(region);
    Client::new(&connection, server.url())
        .unwrap()
        .with_pace(Duration::from_millis(1))
}

fn spec() -> MachineSpec {
    MachineSpec {
        name: "ladakh-otter".into(),
        image: "ghcr.io/fwdai/fletch-cloud:test".into(),
        env: vec![
            (
                "FLETCH_AUTHORIZED_KEY".into(),
                "ssh-ed25519 AAAA engine".into(),
            ),
            ("FLETCH_HOME".into(), "/home/agent".into()),
        ],
        cpus: 2,
        memory_mib: 4096,
        disk_gib: Some(20),
        ssh_port: 2222,
    }
}

/// Fly answering a create that goes right, and deleting whatever it is asked
/// to.
fn fly_ok(r: &Request) -> Reply {
    let path = r.path.as_str();
    match r.method.as_str() {
        "POST" if path == "/v1/apps" => {
            Reply::json(201, json!({ "id": r.body["name"], "created_at": 1 }))
        }
        "POST" if path.ends_with("/ip_assignments") => {
            Reply::json(200, json!({ "ip": "66.241.124.1", "shared": true }))
        }
        "POST" if path.ends_with("/machines") => Reply::json(
            200,
            json!({ "id": "m1", "state": "created", "instance_id": "v1", "version": "v1" }),
        ),
        "GET" if path.contains("/machines/m1/wait?") => {
            Reply::json(200, json!({ "ok": true, "state": "started" }))
        }
        "DELETE" => Reply::empty(202),
        _ => Reply::json(500, json!({ "error": format!("unexpected {}", r.line()) })),
    }
}

/// The app name a create chose, from its first request.
fn created_app(server: &StandIn) -> String {
    server.requests()[0].body["name"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn create_makes_the_app_its_addresses_and_its_machine_then_waits_for_it() {
    let server = StandIn::start(fly_ok).await;
    let created = client(&server, TOKEN, None).create(&spec()).await.unwrap();

    let Handle::Fly { app, machine } = &created.handle;
    assert!(app.starts_with("fletch-box-ladakh-otter-"), "{app}");
    assert_eq!(machine, "m1");
    assert_eq!(
        created.endpoint,
        Endpoint::Tls {
            host: format!("{app}.fly.dev"),
            port: 443
        }
    );
    assert_eq!(created.handle.endpoint(), created.endpoint);
    assert_eq!(
        server.lines(),
        [
            "POST /v1/apps".to_string(),
            format!("POST /v1/apps/{app}/ip_assignments"),
            format!("POST /v1/apps/{app}/ip_assignments"),
            format!("POST /v1/apps/{app}/machines"),
            format!("GET /v1/apps/{app}/machines/m1/wait?state=started&timeout=30"),
        ]
    );

    let requests = server.requests();
    for r in &requests {
        assert_eq!(
            r.header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str()),
            "{}",
            r.line()
        );
    }
    assert_eq!(
        requests[0].body,
        json!({ "name": app, "app_name": app, "org_slug": "acme" })
    );
    assert_eq!(requests[1].body, json!({ "type": "shared_v4" }));
    assert_eq!(requests[2].body, json!({ "type": "v6" }));
    assert_eq!(
        requests[3].body,
        json!({
            "name": app,
            "config": {
                "image": "ghcr.io/fwdai/fletch-cloud:test",
                "env": {
                    "FLETCH_AUTHORIZED_KEY": "ssh-ed25519 AAAA engine",
                    "FLETCH_HOME": "/home/agent",
                },
                "guest": {
                    "cpu_kind": "shared",
                    "cpus": 2,
                    "memory_mb": 4096,
                    "persist_rootfs": "always",
                },
                "rootfs": { "persist": "always", "size_gb": 20 },
                "restart": { "policy": "no" },
                "auto_destroy": false,
                "metadata": { "fletch_box": "ladakh-otter" },
                "services": [{
                    "protocol": "tcp",
                    "internal_port": 2222,
                    "autostart": false,
                    "autostop": "off",
                    "ports": [{ "port": 443, "handlers": ["tls"] }],
                }],
            },
        }),
        "no region stored, so none is sent and Fly chooses"
    );
}

#[tokio::test]
async fn a_machine_fly_cannot_place_takes_its_app_with_it_and_names_the_region() {
    let server = StandIn::start(|r| {
        if r.method == "POST" && r.path.ends_with("/machines") {
            return Reply::json(
                412,
                json!({ "error": "insufficient resources", "status": "insufficient_capacity" }),
            );
        }
        fly_ok(r)
    })
    .await;
    let err = client(&server, TOKEN, Some("ams"))
        .create(&spec())
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("Fly.io has no capacity in ams right now"),
        "{err}"
    );
    let app = created_app(&server);
    assert_eq!(server.requests()[3].body["region"], "ams");
    let lines = server.lines();
    assert_eq!(lines.len(), 5, "{lines:?}");
    assert_eq!(lines[4], format!("DELETE /v1/apps/{app}?force=true"));
}

#[tokio::test]
async fn a_machine_that_fails_to_start_takes_its_app_with_it() {
    let server = StandIn::start(|r| {
        if r.path.contains("/wait?") {
            return Reply::json(
                400,
                json!({ "error": "machine failed to reach desired state" }),
            );
        }
        if r.method == "GET" && r.path.ends_with("/machines/m1") {
            return Reply::json(200, json!({ "id": "m1", "state": "failed" }));
        }
        fly_ok(r)
    })
    .await;
    let err = client(&server, TOKEN, None)
        .create(&spec())
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("did not start"), "{err}");
    let app = created_app(&server);
    assert_eq!(
        server.lines()[4..],
        [
            format!("GET /v1/apps/{app}/machines/m1/wait?state=started&timeout=30"),
            format!("GET /v1/apps/{app}/machines/m1"),
            format!("DELETE /v1/apps/{app}?force=true"),
        ]
    );
}

#[tokio::test]
async fn an_address_fly_will_not_give_takes_the_app_with_it_before_any_machine() {
    let server = StandIn::start(|r| {
        if r.path.ends_with("/ip_assignments") {
            return Reply::json(422, json!({ "error": "billing restriction" }));
        }
        fly_ok(r)
    })
    .await;
    let err = client(&server, TOKEN, None)
        .create(&spec())
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("give the app an address (shared_v4)"), "{err}");
    assert!(err.contains("billing restriction"), "{err}");
    let app = created_app(&server);
    assert_eq!(
        server.lines(),
        [
            "POST /v1/apps".to_string(),
            format!("POST /v1/apps/{app}/ip_assignments"),
            format!("DELETE /v1/apps/{app}?force=true"),
        ]
    );
}

/// A refused app create made nothing — and a name taken is someone else's app,
/// which must never be deleted. A server error may have made the app, so it is
/// deleted.
#[tokio::test]
async fn an_app_create_is_cleaned_up_only_when_it_may_have_happened() {
    let taken = StandIn::start(|_| {
        Reply::json(
            422,
            json!({ "error": "name taken", "status": "name_taken" }),
        )
    })
    .await;
    let err = client(&taken, TOKEN, None)
        .create(&spec())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("refused to create an app"),
        "{err}"
    );
    assert_eq!(taken.lines(), ["POST /v1/apps"]);

    let failing = StandIn::start(|r| match r.method.as_str() {
        "DELETE" => Reply::json(404, json!({ "error": "app not found" })),
        _ => Reply::json(503, json!({ "error": "upstream unavailable" })),
    })
    .await;
    let err = client(&failing, TOKEN, None)
        .create(&spec())
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("Fly.io failed to create an app (HTTP 503)"),
        "{err}"
    );
    let app = created_app(&failing);
    assert_eq!(
        failing.lines(),
        [
            "POST /v1/apps".to_string(),
            format!("DELETE /v1/apps/{app}?force=true")
        ]
    );
}

#[tokio::test]
async fn destroy_deletes_the_app_and_a_gone_one_is_success() {
    let server = StandIn::start(|r| match r.path.as_str() {
        p if p.starts_with("/v1/apps/fletch-box-gone") => {
            Reply::json(404, json!({ "error": "app not found" }))
        }
        _ => Reply::empty(202),
    })
    .await;
    let fly = client(&server, TOKEN, None);
    fly.destroy(APP).await.unwrap();
    fly.destroy("fletch-box-gone-abc").await.unwrap();
    assert_eq!(
        server.lines(),
        [
            format!("DELETE /v1/apps/{APP}?force=true"),
            "DELETE /v1/apps/fletch-box-gone-abc?force=true".to_string(),
        ]
    );
}

#[tokio::test]
async fn destroy_never_touches_an_app_that_is_not_a_box() {
    let server = StandIn::start(|_| Reply::empty(202)).await;
    let err = client(&server, TOKEN, None)
        .destroy("my-production-api")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("refusing to delete it"), "{err}");
    assert!(server.lines().is_empty());
}

#[test]
fn every_documented_fly_state_maps_onto_a_box_state() {
    use MachineState::*;
    let table = [
        // Persistent.
        ("created", Stopped),
        ("started", Running),
        ("stopped", Stopped),
        ("suspended", Stopped),
        ("archived", Stopped),
        // Transient.
        ("creating", Starting),
        ("starting", Starting),
        ("restarting", Starting),
        ("updating", Starting),
        ("replacing", Starting),
        ("pending", Starting),
        ("migrating", Starting),
        ("stopping", Stopping),
        ("suspending", Stopping),
        ("destroying", Gone),
        // Terminal.
        ("destroyed", Gone),
        ("replaced", Starting),
        ("migrated", Starting),
    ];
    for (fly, want) in table {
        assert_eq!(machine_state(fly), want, "{fly}");
    }
    for fly in ["failed", "launch_failed"] {
        assert!(matches!(machine_state(fly), Failed(_)), "{fly}");
    }
    for unknown in ["hibernating", ""] {
        assert!(
            matches!(machine_state(unknown), Failed(reason) if reason.contains(&format!("{unknown:?}"))),
            "an unknown state names itself: {unknown:?}"
        );
    }
}

#[tokio::test]
async fn state_reads_the_machine_and_a_missing_one_is_gone() {
    let server = StandIn::start(|r| match r.path.as_str() {
        p if p.ends_with("/machines/m1") => {
            Reply::json(200, json!({ "id": "m1", "state": "started" }))
        }
        _ => Reply::json(404, json!({ "error": "machine not found" })),
    })
    .await;
    let fly = client(&server, TOKEN, None);
    assert_eq!(fly.state(APP, "m1").await.unwrap(), MachineState::Running);
    assert_eq!(fly.state(APP, "m2").await.unwrap(), MachineState::Gone);
    // An app listed with no machine is gone without asking.
    assert_eq!(fly.state(APP, "").await.unwrap(), MachineState::Gone);
    assert_eq!(
        server.lines(),
        [
            format!("GET /v1/apps/{APP}/machines/m1"),
            format!("GET /v1/apps/{APP}/machines/m2"),
        ]
    );
}

#[tokio::test]
async fn stop_asks_then_waits_for_stopped_naming_the_version() {
    let server = StandIn::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("GET", p) if p.ends_with("/machines/m1") => Reply::json(
            200,
            json!({ "id": "m1", "state": "started", "version": "v7" }),
        ),
        ("POST", p) if p.ends_with("/stop") => Reply::json(200, json!({ "ok": true })),
        ("GET", p) if p.contains("/wait?") => Reply::json(200, json!({ "ok": true })),
        _ => Reply::json(500, json!({})),
    })
    .await;
    client(&server, TOKEN, None).stop(APP, "m1").await.unwrap();
    assert_eq!(
        server.lines(),
        [
            format!("GET /v1/apps/{APP}/machines/m1"),
            format!("POST /v1/apps/{APP}/machines/m1/stop"),
            format!("GET /v1/apps/{APP}/machines/m1/wait?state=stopped&timeout=30&instance_id=v7"),
        ]
    );
    assert_eq!(
        server.requests()[1].body,
        serde_json::Value::Null,
        "no body"
    );
}

#[tokio::test]
async fn stop_and_start_leave_a_machine_already_there_alone() {
    let server = StandIn::start(|r| match r.path.as_str() {
        p if p.ends_with("/machines/up") => Reply::json(200, json!({ "state": "started" })),
        p if p.ends_with("/machines/down") => Reply::json(200, json!({ "state": "stopped" })),
        _ => Reply::json(404, json!({})),
    })
    .await;
    let fly = client(&server, TOKEN, None);
    fly.start(APP, "up").await.unwrap();
    fly.stop(APP, "down").await.unwrap();
    let gone = fly.start(APP, "missing").await.unwrap_err();
    assert!(matches!(gone, Error::CloudGone(_)), "{gone}");
    assert_eq!(
        server.lines(),
        [
            format!("GET /v1/apps/{APP}/machines/up"),
            format!("GET /v1/apps/{APP}/machines/down"),
            format!("GET /v1/apps/{APP}/machines/missing"),
        ]
    );
}

/// A wait that comes back unreached (Fly's 408 when its timeout passes) is
/// followed by a read of the machine's state, which settles it.
#[tokio::test]
async fn start_asks_then_waits_reading_the_state_when_a_wait_times_out() {
    let reads = Arc::new(AtomicUsize::new(0));
    let seen = reads.clone();
    let server = StandIn::start(move |r| match (r.method.as_str(), r.path.as_str()) {
        ("GET", p) if p.ends_with("/machines/m1") => {
            let state = if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                "stopped"
            } else {
                "started"
            };
            Reply::json(200, json!({ "id": "m1", "state": state }))
        }
        ("POST", p) if p.ends_with("/start") => {
            Reply::json(200, json!({ "previous_state": "stopped" }))
        }
        ("GET", p) if p.contains("/wait?") => {
            Reply::json(408, json!({ "error": "deadline_exceeded" }))
        }
        _ => Reply::json(500, json!({})),
    })
    .await;
    client(&server, TOKEN, None).start(APP, "m1").await.unwrap();
    assert_eq!(
        server.lines(),
        [
            format!("GET /v1/apps/{APP}/machines/m1"),
            format!("POST /v1/apps/{APP}/machines/m1/start"),
            format!("GET /v1/apps/{APP}/machines/m1/wait?state=started&timeout=30"),
            format!("GET /v1/apps/{APP}/machines/m1"),
        ]
    );
    assert_eq!(reads.load(Ordering::SeqCst), 2);
}

/// A start Fly refuses stands only if the machine is not already starting.
#[tokio::test]
async fn a_refused_start_of_a_machine_already_starting_is_waited_on() {
    let server = StandIn::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("GET", p) if p.ends_with("/machines/m1") => {
            Reply::json(200, json!({ "id": "m1", "state": "starting" }))
        }
        ("GET", p) if p.ends_with("/machines/m2") => {
            Reply::json(200, json!({ "id": "m2", "state": "stopped" }))
        }
        ("POST", _) => Reply::json(412, json!({ "error": "machine is busy" })),
        ("GET", p) if p.contains("/wait?") => Reply::json(200, json!({ "ok": true })),
        _ => Reply::json(500, json!({})),
    })
    .await;
    let fly = client(&server, TOKEN, None);
    // Starting already: no request needed, only the wait.
    fly.start(APP, "m1").await.unwrap();
    // Stopped and refused: the refusal stands.
    let err = fly.start(APP, "m2").await.unwrap_err().to_string();
    assert!(
        err.contains("refused to start the machine (HTTP 412): machine is busy"),
        "{err}"
    );
}

#[tokio::test]
async fn exec_runs_the_argv_with_an_explicit_timeout_and_returns_what_it_did() {
    let server = StandIn::start(|r| match r.body["command"][1].as_str() {
        Some("ok") => Reply::json(
            200,
            json!({ "exit_code": 0, "exit_signal": 0, "stdout": "ssh-ed25519 AAAAC3 host\n", "stderr": "" }),
        ),
        Some("fails") => Reply::json(200, json!({ "exit_code": 1, "stderr": "cat: fails: No such file" })),
        Some("killed") => Reply::json(200, json!({ "exit_code": 0, "exit_signal": 9 })),
        _ => Reply::json(500, json!({})),
    })
    .await;
    let fly = client(&server, TOKEN, None);
    let argv = |arg: &str| vec!["cat".to_string(), arg.to_string()];

    let out = fly
        .exec(APP, "m1", &argv("ok"), Duration::from_secs(20))
        .await
        .unwrap();
    assert_eq!(
        out,
        ExecOutput {
            exit_code: 0,
            stdout: "ssh-ed25519 AAAAC3 host\n".into(),
            stderr: String::new()
        }
    );
    let failed = fly
        .exec(APP, "m1", &argv("fails"), Duration::from_secs(3600))
        .await
        .unwrap();
    assert_eq!(failed.exit_code, 1, "a command that ran and failed is Ok");
    assert_eq!(failed.stderr, "cat: fails: No such file");
    let killed = fly
        .exec(APP, "m1", &argv("killed"), Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(killed.exit_code, 137, "SIGKILL reads as 128 + 9");

    let requests = server.requests();
    assert_eq!(
        requests[0].line(),
        format!("POST /v1/apps/{APP}/machines/m1/exec")
    );
    assert_eq!(
        requests[0].body,
        json!({ "command": ["cat", "ok"], "timeout": 20 })
    );
    assert_eq!(requests[1].body["timeout"], 600, "clamped to Fly's maximum");
    assert_eq!(requests[2].body["timeout"], 1, "at least a second");
}

#[tokio::test]
async fn exec_in_a_machine_not_running_or_gone_is_an_error() {
    let server = StandIn::start(|r| match r.path.as_str() {
        p if p.contains("/machines/stopped/") => {
            Reply::json(412, json!({ "error": "machine not running" }))
        }
        _ => Reply::json(404, json!({ "error": "machine not found" })),
    })
    .await;
    let fly = client(&server, TOKEN, None);
    let argv = ["true".to_string()];
    let err = fly
        .exec(APP, "stopped", &argv, Duration::from_secs(5))
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("could not run true in the machine stopped: it is not running"),
        "{err}"
    );
    let err = fly
        .exec(APP, "deleted", &argv, Duration::from_secs(5))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::CloudGone(_)), "{err}");
}

#[tokio::test]
async fn validate_lists_the_orgs_apps_with_the_token_as_fly_issued_it() {
    let server = StandIn::start(|_| Reply::json(200, json!({ "total_apps": 0, "apps": [] }))).await;
    client(&server, FLYV1, None).validate().await.unwrap();
    client(&server, TOKEN, None).validate().await.unwrap();
    let requests = server.requests();
    assert_eq!(requests[0].line(), "GET /v1/apps?org_slug=acme");
    assert_eq!(
        requests[0].header("authorization"),
        Some(FLYV1),
        "FlyV1 tokens go as they are"
    );
    assert_eq!(
        requests[1].header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
}

#[tokio::test]
async fn a_refused_token_and_an_unknown_org_say_different_things() {
    let refused = StandIn::start(|_| Reply::json(401, json!({ "error": "unauthorized" }))).await;
    let err = client(&refused, FLYV1, None)
        .validate()
        .await
        .unwrap_err()
        .to_string();
    assert!(err.starts_with("Fly.io refused the token"), "{err}");
    assert!(!err.contains("fm2_"), "{err}");

    let no_org =
        StandIn::start(|_| Reply::json(404, json!({ "error": "organization not found" }))).await;
    let err = client(&no_org, TOKEN, None)
        .validate()
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(
            "Fly.io has no organization \"acme\" this token can use: organization not found"
        ),
        "{err}"
    );
}

#[tokio::test]
async fn a_rate_limited_request_is_retried_with_backoff() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let server = StandIn::start(move |_| {
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::json(429, json!({ "error": "rate limited" })).header("Retry-After", "0")
        } else {
            Reply::json(200, json!({ "apps": [] }))
        }
    })
    .await;
    client(&server, TOKEN, None).validate().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_rate_limit_that_persists_is_reported_after_a_bounded_number_of_tries() {
    let server = StandIn::start(|_| Reply::json(429, json!({ "error": "rate limited" }))).await;
    let err = client(&server, TOKEN, None)
        .stop(APP, "m1")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Fly.io is rate-limiting Fletch's requests"),
        "{err}"
    );
    assert_eq!(server.lines().len(), 4, "the request and three retries");
}

#[tokio::test]
async fn list_keeps_the_box_apps_and_their_labelled_machines() {
    let server = StandIn::start(|r| match r.path.as_str() {
        "/v1/apps?org_slug=acme" => Reply::json(
            200,
            json!({ "total_apps": 5, "apps": [
                { "name": "fletch-box-a-1", "machine_count": 1, "volume_count": 0 },
                { "name": "fletch-box-b-2", "machine_count": 0, "volume_count": 0 },
                { "name": "fletch-box-c-3", "machine_count": 1, "volume_count": 0 },
                { "name": "fletch-box-d-4", "machine_count": 0, "volume_count": 1 },
                { "name": "my-api", "machine_count": 2, "volume_count": 0 },
                { "name": "fletch-box-e-5", "machine_count": 1, "volume_count": 0 },
            ]}),
        ),
        "/v1/apps/fletch-box-a-1/machines" => Reply::json(
            200,
            json!([{ "id": "ma", "state": "stopped", "config": { "metadata": { "fletch_box": "a" } } }]),
        ),
        "/v1/apps/fletch-box-c-3/machines" => Reply::json(
            200,
            json!([{ "id": "mc", "state": "started", "config": { "metadata": {} } }]),
        ),
        "/v1/apps/fletch-box-e-5/machines" => Reply::json(
            200,
            json!([{ "id": "me", "state": "started", "config": null,
                     "incomplete_config": { "metadata": { "fletch_box": "e" } } }]),
        ),
        p if p.ends_with("/machines") => Reply::json(200, json!([])),
        _ => Reply::json(500, json!({})),
    })
    .await;
    let listed = client(&server, TOKEN, None).list().await.unwrap();
    let fly = |app: &str, machine: &str| Handle::Fly {
        app: app.into(),
        machine: machine.into(),
    };
    assert_eq!(
        listed,
        [
            Listed {
                handle: fly("fletch-box-a-1", "ma"),
                state: MachineState::Stopped
            },
            Listed {
                handle: fly("fletch-box-b-2", ""),
                state: MachineState::Gone
            },
            Listed {
                handle: fly("fletch-box-e-5", "me"),
                state: MachineState::Running
            },
        ],
        "an unlabelled machine, an app with a volume and an app without the prefix are not ours"
    );
    assert!(
        !server.lines().iter().any(|l| l.contains("my-api")),
        "an app without the prefix is never even read"
    );
}

#[test]
fn an_app_name_is_the_box_name_folded_into_flys_rules() {
    assert_eq!(
        fold_app_name("Ladakh_Otter 42!!", "abcd1234"),
        "fletch-box-ladakh-otter-42-abcd1234"
    );
    assert_eq!(fold_app_name("--__--", "abcd1234"), "fletch-box-abcd1234");
    assert_eq!(fold_app_name("", "abcd1234"), "fletch-box-abcd1234");
    assert_eq!(
        fold_app_name("Ünïcode-name", "abcd1234"),
        "fletch-box-n-code-name-abcd1234"
    );
    let long = fold_app_name(&"a".repeat(100), "abcd1234");
    assert_eq!(long.len(), 63, "one DNS label");
    // Cut where a dash would end the folded name: the dash goes.
    let cut = fold_app_name(&format!("{}-bbb", "a".repeat(42)), "abcd1234");
    assert_eq!(cut, format!("fletch-box-{}-abcd1234", "a".repeat(42)));

    let (one, two) = (app_name("box"), app_name("box"));
    assert_ne!(one, two, "the random suffix makes each name unique");
    for name in [one, two] {
        assert!(name.starts_with("fletch-box-box-"), "{name}");
        assert_eq!(name.len(), "fletch-box-box-".len() + 8);
        assert!(name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    }
}

/// Records every event's fields, so a test can read what was logged.
struct Recorder(Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for Recorder {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(String);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.push_str(&format!("{}={value:?} ", field.name()));
            }
        }
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        self.0.lock().unwrap().push(fields.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Fly's error text is quoted in messages, so a Fly that echoed the token back
/// would put it in every error and log line built from one — unless it is
/// taken out. Nothing produced here may hold any piece of the token.
///
/// Also the one test of a create whose cleanup fails too, and the only one
/// that may reach that warning: `tracing` caches a callsite's interest for the
/// whole process, so another test reaching it first, on a thread with no
/// subscriber, could leave it silenced for the recorder here.
#[tokio::test]
async fn no_error_or_log_line_holds_the_token() {
    let logged = Arc::new(Mutex::new(Vec::new()));
    let _guard = tracing::subscriber::set_default(Recorder(logged.clone()));
    tracing::callsite::rebuild_interest_cache();
    let server = StandIn::start(|r| {
        let echo = json!({ "error": format!("bad credentials {FLYV1}") });
        match r.method.as_str() {
            "POST" if r.path.ends_with("/machines") => Reply::json(400, echo),
            "DELETE" => Reply::json(500, json!({ "error": "fm2_macaroonB9876543210 expired" })),
            "GET" if r.path.starts_with("/v1/apps?") => Reply::json(401, echo),
            "POST" if r.path.ends_with("/exec") => Reply::json(412, echo),
            _ => fly_ok(r),
        }
    })
    .await;
    let fly = client(&server, FLYV1, None);
    let errors = [
        fly.create(&spec()).await.unwrap_err().to_string(),
        fly.validate().await.unwrap_err().to_string(),
        fly.list().await.unwrap_err().to_string(),
        fly.exec(APP, "m1", &["true".into()], Duration::from_secs(1))
            .await
            .unwrap_err()
            .to_string(),
    ];
    let app = created_app(&server);
    assert!(
        errors[0].contains(&format!(
            "the Fly.io app {app} it left could not be deleted"
        )),
        "a cleanup that fails too names the app it left: {}",
        errors[0]
    );
    assert!(
        errors[0].contains("delete it in the Fly.io dashboard"),
        "{}",
        errors[0]
    );
    let logged = logged.lock().unwrap().clone();
    assert!(
        logged
            .iter()
            .any(|l| l.contains("could not delete") && l.contains(&app)),
        "the failed cleanup is logged: {logged:?}"
    );
    for line in errors.iter().chain(&logged) {
        for piece in ["fm2_macaroonA0123456789", "fm2_macaroonB9876543210", FLYV1] {
            assert!(!line.contains(piece), "{line}");
        }
    }
    assert!(errors[0].contains("[token]"), "{}", errors[0]);
}
