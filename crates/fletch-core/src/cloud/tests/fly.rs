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
const ORG: &str = "acme";
const APP: &str = "fletch-box-ladakh-otter-k3j9x2ab";
const VOL: &str = "vol_0123456789abcdef";

fn client(server: &StandIn, token: &str, region: Option<&str>) -> Client {
    let connection = Connection::new(ProviderId::Fly, token, Some(ORG))
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
                "FLETCH_BOX_AUTHORIZED_KEY".into(),
                "ssh-ed25519 AAAA engine".into(),
            ),
            ("FLETCH_BOX_HOME".into(), "/Users/alex".into()),
        ],
        cpus: 2,
        memory_mib: 4096,
        disk_gib: Some(20),
        ssh_port: 2222,
    }
}

/// The create-machine request a box with [`spec`] sends, on [`VOL`] in
/// `region`.
fn machine_request(app: &str, region: &str) -> serde_json::Value {
    json!({
        "name": app,
        "region": region,
        "config": {
            "image": "ghcr.io/fwdai/fletch-cloud:test",
            "env": {
                "FLETCH_BOX_AUTHORIZED_KEY": "ssh-ed25519 AAAA engine",
                "FLETCH_BOX_HOME": "/Users/alex",
            },
            "guest": {
                "cpu_kind": "shared",
                "cpus": 2,
                "memory_mb": 4096,
                "persist_rootfs": "restart",
            },
            "rootfs": { "persist": "restart" },
            "mounts": [{ "volume": VOL, "path": "/data" }],
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
    })
}

/// Fly answering a create that goes right: the volume lands in the region
/// asked for, or `iad` (the API server's) without one. An app's lists of
/// machines and volumes are empty, and whatever is deleted goes.
fn fly_ok(r: &Request) -> Reply {
    let path = r.path.as_str();
    match r.method.as_str() {
        "POST" if path == "/v1/apps" => {
            Reply::json(201, json!({ "id": r.body["name"], "created_at": 1 }))
        }
        "POST" if path.ends_with("/ip_assignments") => {
            Reply::json(200, json!({ "ip": "66.241.124.1", "shared": true }))
        }
        "POST" if path.ends_with("/volumes") => Reply::json(
            200,
            json!({
                "id": VOL,
                "name": "fletch_data",
                "state": "created",
                "region": r.body["region"].as_str().unwrap_or("iad"),
                "size_gb": r.body["size_gb"],
            }),
        ),
        "POST" if path.ends_with("/machines") => Reply::json(
            200,
            json!({ "id": "m1", "state": "created", "instance_id": "v1", "version": "v1" }),
        ),
        "GET" if path.contains("/wait?") => {
            Reply::json(200, json!({ "ok": true, "state": "started" }))
        }
        "GET" if path.ends_with("/machines") || path.ends_with("/volumes") => {
            Reply::json(200, json!([]))
        }
        "DELETE" => Reply::json(200, json!({ "ok": true })),
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
async fn create_makes_the_app_its_addresses_its_disk_and_a_machine_on_the_disk() {
    let server = StandIn::start(fly_ok).await;
    let created = client(&server, TOKEN, None).create(&spec()).await.unwrap();

    let Handle::Fly {
        org,
        app,
        volume,
        machine,
    } = &created.handle;
    assert_eq!(org, ORG);
    assert!(app.starts_with("fletch-box-ladakh-otter-"), "{app}");
    assert_eq!((volume.as_str(), machine.as_str()), (VOL, "m1"));
    assert_eq!(
        created.endpoint,
        Endpoint::Tls {
            host: format!("{app}.fly.dev"),
            port: 443
        }
    );
    assert_eq!(
        server.lines(),
        [
            "POST /v1/apps".to_string(),
            format!("POST /v1/apps/{app}/ip_assignments"),
            format!("POST /v1/apps/{app}/ip_assignments"),
            format!("POST /v1/apps/{app}/volumes"),
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
    assert_eq!(requests[0].body, json!({ "name": app, "org_slug": "acme" }));
    assert_eq!(requests[1].body, json!({ "type": "shared_v4" }));
    assert_eq!(requests[2].body, json!({ "type": "v6" }));
    assert_eq!(
        requests[3].body,
        json!({
            "name": "fletch_data",
            "size_gb": 20,
            "fstype": "ext4",
            "encrypted": true,
            "auto_backup_enabled": true,
            "snapshot_retention": 5,
            "require_unique_zone": false,
            "compute": { "cpu_kind": "shared", "cpus": 2, "memory_mb": 4096 },
        }),
        "no region stored: none is sent, and Fly places the disk"
    );
    assert_eq!(
        requests[4].body,
        machine_request(app, "iad"),
        "the machine goes to the region Fly put the disk in, mounting it at /data"
    );
}

#[tokio::test]
async fn a_stored_region_places_the_disk_and_the_machine_follows_it() {
    let server = StandIn::start(fly_ok).await;
    let mut spec = spec();
    spec.disk_gib = None;
    client(&server, TOKEN, Some("ams"))
        .create(&spec)
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests[3].body["region"], "ams");
    assert_eq!(
        requests[3].body["size_gb"], 8,
        "no size given: a local box's default"
    );
    assert_eq!(requests[4].body["region"], "ams");
}

/// Everything after the app cleans up by destroying the app with what it
/// holds, in the order Fly allows: machines, then volumes, then the app.
#[tokio::test]
async fn a_disk_fly_cannot_place_takes_the_app_with_it() {
    let server = StandIn::start(|r| {
        if r.method == "POST" && r.path.ends_with("/volumes") {
            return Reply::json(
                412,
                json!({ "error": "no capacity", "status": "insufficient_capacity" }),
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
    assert_eq!(
        server.lines()[4..],
        [
            format!("GET /v1/apps/{app}/machines"),
            format!("GET /v1/apps/{app}/volumes"),
            format!("DELETE /v1/apps/{app}?force=true"),
        ]
    );
}

#[tokio::test]
async fn a_machine_fly_cannot_place_on_the_disks_host_takes_the_disk_and_the_app_with_it() {
    let server = StandIn::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("POST", p) if p.ends_with("/machines") => Reply::json(
            412,
            json!({ "error": "insufficient resources", "status": "insufficient_capacity" }),
        ),
        ("GET", p) if p.ends_with("/volumes") => Reply::json(
            200,
            json!([{ "id": VOL, "name": "fletch_data", "state": "created" }]),
        ),
        _ => fly_ok(r),
    })
    .await;
    let err = client(&server, TOKEN, Some("ams"))
        .create(&spec())
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("no room right now on the host that holds this box's disk (in ams)"),
        "{err}"
    );
    let app = created_app(&server);
    assert_eq!(
        server.lines()[5..],
        [
            format!("GET /v1/apps/{app}/machines"),
            format!("GET /v1/apps/{app}/volumes"),
            format!("DELETE /v1/apps/{app}/volumes/{VOL}"),
            format!("DELETE /v1/apps/{app}?force=true"),
        ],
        "the disk is deleted before the app, so none is left whatever the app delete does"
    );
}

#[tokio::test]
async fn a_machine_that_fails_to_start_takes_itself_its_disk_and_the_app_with_it() {
    let server = StandIn::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("GET", p) if p.contains("/wait?state=started") => Reply::json(
            400,
            json!({ "error": "machine failed to reach desired state" }),
        ),
        ("GET", p) if p.ends_with("/machines/m1") => {
            Reply::json(200, json!({ "id": "m1", "state": "failed" }))
        }
        ("GET", p) if p.ends_with("/machines") => {
            Reply::json(200, json!([{ "id": "m1", "state": "failed" }]))
        }
        ("GET", p) if p.ends_with("/volumes") => Reply::json(
            200,
            json!([{ "id": VOL, "name": "fletch_data", "state": "created" }]),
        ),
        _ => fly_ok(r),
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
        server.lines()[5..],
        [
            format!("GET /v1/apps/{app}/machines/m1/wait?state=started&timeout=30"),
            format!("GET /v1/apps/{app}/machines/m1"),
            format!("GET /v1/apps/{app}/machines"),
            format!("DELETE /v1/apps/{app}/machines/m1?force=true"),
            format!("GET /v1/apps/{app}/machines/m1/wait?state=destroyed&timeout=30"),
            format!("GET /v1/apps/{app}/volumes"),
            format!("DELETE /v1/apps/{app}/volumes/{VOL}"),
            format!("DELETE /v1/apps/{app}?force=true"),
        ]
    );
}

#[tokio::test]
async fn an_address_fly_will_not_give_takes_the_app_with_it_before_any_disk() {
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
            format!("GET /v1/apps/{app}/machines"),
            format!("GET /v1/apps/{app}/volumes"),
            format!("DELETE /v1/apps/{app}?force=true"),
        ]
    );
}

/// A refused app create made nothing — and a name taken is someone else's app,
/// which must never be touched. A server error may have made the app, so it is
/// looked for and deleted if there.
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
        "GET" => Reply::json(404, json!({ "error": "app not found" })),
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
            format!("GET /v1/apps/{app}/machines"),
        ],
        "the app was never made: nothing to delete"
    );
}

#[tokio::test]
async fn destroy_deletes_the_machine_then_the_disk_then_the_app_and_a_gone_box_is_success() {
    let server = StandIn::start(|r| match (r.method.as_str(), r.path.as_str()) {
        (_, p) if p.starts_with("/v1/apps/fletch-box-gone") => {
            Reply::json(404, json!({ "error": "app not found" }))
        }
        ("GET", p) if p.ends_with("/machines") => {
            Reply::json(200, json!([{ "id": "m1", "state": "stopped" }]))
        }
        ("GET", p) if p.ends_with("/volumes") => Reply::json(
            200,
            json!([
                { "id": VOL, "name": "fletch_data", "state": "created" },
                { "id": "vol_old", "name": "fletch_data", "state": "pending_destroy" },
            ]),
        ),
        ("GET", p) if p.contains("/wait?") => Reply::json(404, json!({})),
        ("DELETE", p) if p.ends_with(&format!("/apps/{APP}")) => Reply::empty(202),
        ("DELETE", _) => Reply::json(200, json!({ "ok": true })),
        _ => Reply::json(500, json!({})),
    })
    .await;
    let fly = client(&server, TOKEN, None);
    fly.destroy(APP).await.unwrap();
    fly.destroy("fletch-box-gone-abc").await.unwrap();
    assert_eq!(
        server.lines(),
        [
            format!("GET /v1/apps/{APP}/machines"),
            format!("DELETE /v1/apps/{APP}/machines/m1?force=true"),
            format!("GET /v1/apps/{APP}/machines/m1/wait?state=destroyed&timeout=30"),
            format!("GET /v1/apps/{APP}/volumes"),
            format!("DELETE /v1/apps/{APP}/volumes/{VOL}"),
            format!("DELETE /v1/apps/{APP}?force=true"),
            "GET /v1/apps/fletch-box-gone-abc/machines".to_string(),
        ],
        "a volume already being destroyed is left to it; an app already gone is done"
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

/// The apps of [`state_reads_the_disk_and_the_machine_and_says_what_is_left`],
/// one per shape a box can be in.
fn fly_shapes(r: &Request) -> Reply {
    let live = json!({ "id": VOL, "name": "fletch_data", "state": "created", "region": "ams" });
    let path = r.path.as_str();
    let app = path.split('/').nth(3).unwrap_or_default();
    match (app, path.rsplit('/').next().unwrap_or_default()) {
        ("fletch-box-whole", "m1") => Reply::json(
            200,
            json!({ "id": "m1", "state": "started", "host_status": "ok" }),
        ),
        ("fletch-box-whole", _) => Reply::json(200, live),
        // A machine destroyed from the dashboard: 404, or still listed destroyed.
        ("fletch-box-no-machine", "m1") => Reply::json(404, json!({ "error": "not found" })),
        ("fletch-box-destroyed-machine", "m1") => {
            Reply::json(200, json!({ "id": "m1", "state": "destroyed" }))
        }
        ("fletch-box-no-machine" | "fletch-box-destroyed-machine", _) => Reply::json(200, live),
        // A volume gone under a machine that is still there.
        ("fletch-box-no-disk", "m1") => Reply::json(200, json!({ "id": "m1", "state": "stopped" })),
        ("fletch-box-no-disk", _) => Reply::json(404, json!({ "error": "volume not found" })),
        // Both gone, the app still there.
        ("fletch-box-empty", "fletch-box-empty") => Reply::json(200, json!({})),
        ("fletch-box-empty", p) if p == VOL => {
            Reply::json(200, json!({ "id": VOL, "state": "destroyed" }))
        }
        ("fletch-box-empty", _) => Reply::json(404, json!({})),
        // A volume on a host Fly cannot reach.
        ("fletch-box-dead-host", "m1") => Reply::json(
            200,
            json!({ "id": "m1", "state": "started", "host_status": "unreachable" }),
        ),
        ("fletch-box-dead-host", _) => Reply::json(
            200,
            json!({ "id": VOL, "state": "created", "host_status": "unreachable" }),
        ),
        // A disk whose host Fly could not ask, and no machine answering: not
        // a lost machine to replace.
        ("fletch-box-unknown-disk-host", "m1") => Reply::json(404, json!({ "error": "not found" })),
        ("fletch-box-unknown-disk-host", _) => Reply::json(
            200,
            json!({ "id": VOL, "state": "created", "region": "ams", "host_status": "unknown" }),
        ),
        // A machine whose host Fly could not ask, on a disk it could: its
        // `started` is only the last state known.
        ("fletch-box-unknown-machine-host", "m1") => Reply::json(
            200,
            json!({ "id": "m1", "state": "started", "host_status": "unknown" }),
        ),
        ("fletch-box-unknown-machine-host", _) => Reply::json(200, live),
        // A status Fly does not document yet.
        ("fletch-box-new-host-status", "m1") => Reply::json(404, json!({ "error": "not found" })),
        ("fletch-box-new-host-status", _) => Reply::json(
            200,
            json!({ "id": VOL, "state": "created", "region": "ams", "host_status": "draining" }),
        ),
        _ => Reply::json(404, json!({ "error": "app not found" })),
    }
}

#[tokio::test]
async fn state_reads_the_disk_and_the_machine_and_says_what_is_left() {
    let server = StandIn::start(fly_shapes).await;
    let fly = client(&server, TOKEN, None);
    let state = |app: &'static str| fly.state(ORG, app, VOL, "m1");

    assert_eq!(
        state("fletch-box-whole").await.unwrap(),
        MachineState::Running
    );
    assert_eq!(
        state("fletch-box-no-machine").await.unwrap(),
        MachineState::MachineGone
    );
    assert_eq!(
        state("fletch-box-destroyed-machine").await.unwrap(),
        MachineState::MachineGone
    );
    assert_eq!(
        state("fletch-box-no-disk").await.unwrap(),
        MachineState::DiskGone,
        "a machine with no disk under it has lost the workspace"
    );
    assert_eq!(
        state("fletch-box-empty").await.unwrap(),
        MachineState::DiskGone
    );
    assert_eq!(state("fletch-box-gone").await.unwrap(), MachineState::Gone);
    assert!(
        matches!(state("fletch-box-dead-host").await.unwrap(), MachineState::Failed(r) if r.contains("cannot reach the host")),
        "a dead host is a failure, not a loss"
    );
    // Every host status but `ok` is a host Fly could not ask: a disk there is
    // not a disk to put a new machine on, and a machine there is not running.
    for app in [
        "fletch-box-unknown-disk-host",
        "fletch-box-unknown-machine-host",
        "fletch-box-new-host-status",
    ] {
        let found = state(app).await.unwrap();
        assert!(
            matches!(&found, MachineState::Failed(r) if r.contains("cannot reach the host")),
            "{app}: {found:?}"
        );
    }

    let lines = server.lines();
    assert_eq!(
        lines[..2],
        [
            format!("GET /v1/apps/fletch-box-whole/volumes/{VOL}"),
            "GET /v1/apps/fletch-box-whole/machines/m1".to_string(),
        ]
    );
    assert!(
        lines.contains(&"GET /v1/apps/fletch-box-gone".to_string()),
        "neither answering, the app itself is asked: {lines:?}"
    );
}

/// Nothing answering a connection to another org than the box's says nothing
/// about the box: this token may not see it.
#[tokio::test]
async fn a_box_from_another_org_that_this_token_cannot_see_is_not_called_gone() {
    let server = StandIn::start(fly_shapes).await;
    let err = client(&server, TOKEN, None)
        .state("personal", "fletch-box-gone", VOL, "m1")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("it was created in \"personal\" — connect Fly.io to \"personal\""),
        "{err}"
    );
}

#[tokio::test]
async fn replace_makes_a_new_machine_on_the_same_disk_in_its_region() {
    let server = StandIn::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("GET", p) if p.ends_with("/machines/m1") => Reply::json(404, json!({})),
        ("GET", p) if p.ends_with(&format!("/volumes/{VOL}")) => Reply::json(
            200,
            json!({ "id": VOL, "state": "created", "region": "ams", "attached_machine_id": null }),
        ),
        ("POST", p) if p.ends_with("/machines") => {
            Reply::json(200, json!({ "id": "m2", "state": "created" }))
        }
        ("GET", p) if p.contains("/machines/m2/wait?") => Reply::json(200, json!({ "ok": true })),
        _ => Reply::json(500, json!({})),
    })
    .await;
    let replaced = client(&server, TOKEN, None)
        .replace_machine(ORG, APP, VOL, "m1", &spec())
        .await
        .unwrap();
    assert_eq!(
        replaced.handle,
        Handle::Fly {
            org: ORG.into(),
            app: APP.into(),
            volume: VOL.into(),
            machine: "m2".into(),
        }
    );
    assert_eq!(replaced.endpoint, endpoint(APP), "the address is the box's");
    assert_eq!(
        server.lines(),
        [
            format!("GET /v1/apps/{APP}/machines/m1"),
            format!("GET /v1/apps/{APP}/volumes/{VOL}"),
            format!("POST /v1/apps/{APP}/machines"),
            format!("GET /v1/apps/{APP}/machines/m2/wait?state=started&timeout=30"),
        ]
    );
    assert_eq!(server.requests()[2].body, machine_request(APP, "ams"));
}

#[tokio::test]
async fn replace_never_puts_a_new_disk_where_the_old_one_was_lost() {
    let server = StandIn::start(|r| match r.path.as_str() {
        p if p.ends_with("/machines/m1") => Reply::json(404, json!({})),
        p if p.ends_with(&format!("/volumes/{VOL}")) => {
            Reply::json(200, json!({ "id": VOL, "state": "destroyed" }))
        }
        _ => Reply::json(500, json!({})),
    })
    .await;
    let err = client(&server, TOKEN, None)
        .replace_machine(ORG, APP, VOL, "m1", &spec())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::CloudGone(m) if m.contains("what was on it is lost")),
        "{err}"
    );
    assert_eq!(
        server.lines(),
        [
            format!("GET /v1/apps/{APP}/machines/m1"),
            format!("GET /v1/apps/{APP}/volumes/{VOL}"),
        ],
        "no volume and no machine is created"
    );
}

#[tokio::test]
async fn replace_refuses_while_the_old_machine_is_there() {
    let server =
        StandIn::start(|_| Reply::json(200, json!({ "id": "m1", "state": "stopped" }))).await;
    let err = client(&server, TOKEN, None)
        .replace_machine(ORG, APP, VOL, "m1", &spec())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("still exists"), "{err}");
    assert_eq!(server.lines(), [format!("GET /v1/apps/{APP}/machines/m1")]);
}

/// A replacement that does not start is deleted — and nothing else: the disk
/// holds the workspace and the app holds the disk.
#[tokio::test]
async fn a_replacement_that_fails_deletes_itself_and_keeps_the_disk() {
    let server = StandIn::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("GET", p) if p.ends_with("/machines/m1") => Reply::json(404, json!({})),
        ("GET", p) if p.ends_with(&format!("/volumes/{VOL}")) => Reply::json(
            200,
            json!({ "id": VOL, "state": "created", "region": "ams" }),
        ),
        ("POST", p) if p.ends_with("/machines") => {
            Reply::json(200, json!({ "id": "m2", "state": "created" }))
        }
        ("GET", p) if p.contains("/wait?state=started") => Reply::json(408, json!({})),
        ("GET", p) if p.ends_with("/machines/m2") => {
            Reply::json(200, json!({ "id": "m2", "state": "failed" }))
        }
        ("DELETE", _) => Reply::json(200, json!({ "ok": true })),
        ("GET", p) if p.contains("/wait?state=destroyed") => Reply::json(200, json!({})),
        _ => Reply::json(500, json!({})),
    })
    .await;
    let err = client(&server, TOKEN, None)
        .replace_machine(ORG, APP, VOL, "m1", &spec())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("did not start"), "{err}");
    let deletes: Vec<String> = server
        .lines()
        .into_iter()
        .filter(|l| l.starts_with("DELETE"))
        .collect();
    assert_eq!(
        deletes,
        [format!("DELETE /v1/apps/{APP}/machines/m2?force=true")]
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

/// A start Fly refuses stands only if the machine is not already starting; a
/// start refused for want of room on the disk's host says so.
#[tokio::test]
async fn a_refused_start_is_waited_on_when_already_starting_and_named_when_the_host_is_full() {
    let server = StandIn::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("GET", p) if p.ends_with("/machines/m1") => {
            Reply::json(200, json!({ "id": "m1", "state": "starting" }))
        }
        ("GET", p) if p.ends_with("/machines/m2") || p.ends_with("/machines/m3") => {
            Reply::json(200, json!({ "state": "stopped" }))
        }
        ("POST", p) if p.contains("/machines/m3/") => Reply::json(
            412,
            json!({ "error": "insufficient resources", "status": "insufficient_capacity" }),
        ),
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
    let err = fly.start(APP, "m3").await.unwrap_err().to_string();
    assert!(
        err.contains("no room right now on the host that holds this box's disk, so it could not start the machine"),
        "{err}"
    );
}

/// Fly answering for one machine, `m1` at version `v7`, whose state is held
/// in `state`: a stop and a start move it as Fly would; a suspend is
/// answered with the status `suspend`, and moves it to `suspended` only when
/// that is a success and `takes` says so; a wait succeeds once the machine
/// is in the state waited for, and otherwise times out as Fly's does.
fn fly_machine(
    state: Arc<Mutex<&'static str>>,
    suspend: u16,
    takes: bool,
) -> impl Fn(&Request) -> Reply + Send + Sync + 'static {
    move |r| {
        let mut state = state.lock().unwrap();
        match (r.method.as_str(), r.path.as_str()) {
            ("GET", p) if p.ends_with("/machines/m1") => {
                Reply::json(200, json!({ "id": "m1", "state": *state, "version": "v7" }))
            }
            ("GET", p) if p.ends_with(&format!("/volumes/{VOL}")) => Reply::json(
                200,
                json!({ "id": VOL, "state": "created", "region": "ams" }),
            ),
            ("POST", p) if p.ends_with("/suspend") => {
                if !(200..300).contains(&suspend) {
                    return Reply::json(suspend, json!({ "error": "machine cannot be suspended" }));
                }
                if takes {
                    *state = "suspended";
                }
                Reply::json(suspend, json!({ "ok": true }))
            }
            ("POST", p) if p.ends_with("/stop") => {
                *state = "stopped";
                Reply::json(200, json!({ "ok": true }))
            }
            ("POST", p) if p.ends_with("/start") => {
                let previous = std::mem::replace(&mut *state, "started");
                Reply::json(200, json!({ "previous_state": previous }))
            }
            ("GET", p) if p.contains(&format!("/wait?state={}&", *state)) => {
                Reply::json(200, json!({ "ok": true, "state": *state }))
            }
            ("GET", p) if p.contains("/wait?") => {
                Reply::json(408, json!({ "error": "deadline_exceeded" }))
            }
            _ => Reply::json(500, json!({ "error": format!("unexpected {}", r.line()) })),
        }
    }
}

#[tokio::test]
async fn park_suspends_then_waits_for_suspended_naming_no_version() {
    let state = Arc::new(Mutex::new("started"));
    let server = StandIn::start(fly_machine(state.clone(), 200, true)).await;
    client(&server, TOKEN, None).park(APP, "m1").await.unwrap();
    assert_eq!(*state.lock().unwrap(), "suspended");
    assert_eq!(
        server.lines(),
        [
            format!("GET /v1/apps/{APP}/machines/m1"),
            format!("POST /v1/apps/{APP}/machines/m1/suspend"),
            format!("GET /v1/apps/{APP}/machines/m1/wait?state=suspended&timeout=30"),
        ]
    );
    assert_eq!(
        server.requests()[1].body,
        serde_json::Value::Null,
        "no body"
    );
}

/// Fly does not document how it turns a suspend down (a machine with more
/// than 2 GB of memory, swap or a schedule): any refusal of a machine still
/// running is a stop instead.
#[tokio::test]
async fn a_suspend_fly_refuses_is_a_stop_instead() {
    for status in [400, 412, 422, 500] {
        let state = Arc::new(Mutex::new("started"));
        let server = StandIn::start(fly_machine(state.clone(), status, true)).await;
        client(&server, TOKEN, None).park(APP, "m1").await.unwrap();
        assert_eq!(*state.lock().unwrap(), "stopped", "{status}");
        assert_eq!(
            server.lines(),
            [
                format!("GET /v1/apps/{APP}/machines/m1"),
                format!("POST /v1/apps/{APP}/machines/m1/suspend"),
                // The refusal stands: the machine is still running.
                format!("GET /v1/apps/{APP}/machines/m1"),
                // The stop, as `stop` makes it.
                format!("GET /v1/apps/{APP}/machines/m1"),
                format!("POST /v1/apps/{APP}/machines/m1/stop"),
                format!(
                    "GET /v1/apps/{APP}/machines/m1/wait?state=stopped&timeout=30&instance_id=v7"
                ),
            ],
            "{status}"
        );
    }
}

/// A suspend Fly accepts and does not carry out leaves the machine running
/// once the wait for it is over: that is a stop too, without waiting out the
/// rest of the bound.
#[tokio::test]
async fn a_suspend_that_does_not_reach_suspended_is_a_stop_instead() {
    let state = Arc::new(Mutex::new("started"));
    let server = StandIn::start(fly_machine(state.clone(), 200, false)).await;
    client(&server, TOKEN, None).park(APP, "m1").await.unwrap();
    assert_eq!(*state.lock().unwrap(), "stopped");
    assert_eq!(
        server.lines(),
        [
            format!("GET /v1/apps/{APP}/machines/m1"),
            format!("POST /v1/apps/{APP}/machines/m1/suspend"),
            format!("GET /v1/apps/{APP}/machines/m1/wait?state=suspended&timeout=30"),
            format!("GET /v1/apps/{APP}/machines/m1"),
            format!("GET /v1/apps/{APP}/machines/m1"),
            format!("POST /v1/apps/{APP}/machines/m1/stop"),
            format!("GET /v1/apps/{APP}/machines/m1/wait?state=stopped&timeout=30&instance_id=v7"),
        ]
    );
}

/// A machine already at rest is asked nothing: a stop would throw a
/// suspended machine's snapshot away.
#[tokio::test]
async fn park_leaves_a_machine_not_running_alone() {
    let server = StandIn::start(|r| match r.path.as_str() {
        p if p.ends_with("/machines/stopped") => Reply::json(200, json!({ "state": "stopped" })),
        p if p.ends_with("/machines/suspended") => {
            Reply::json(200, json!({ "state": "suspended" }))
        }
        p if p.ends_with("/machines/failed") => Reply::json(200, json!({ "state": "failed" })),
        _ => Reply::json(404, json!({})),
    })
    .await;
    let fly = client(&server, TOKEN, None);
    for machine in ["stopped", "suspended", "failed"] {
        fly.park(APP, machine).await.unwrap();
    }
    let gone = fly.park(APP, "missing").await.unwrap_err();
    assert!(matches!(gone, Error::CloudGone(_)), "{gone}");
    assert_eq!(
        server.lines(),
        [
            format!("GET /v1/apps/{APP}/machines/stopped"),
            format!("GET /v1/apps/{APP}/machines/suspended"),
            format!("GET /v1/apps/{APP}/machines/failed"),
            format!("GET /v1/apps/{APP}/machines/missing"),
        ]
    );
}

/// A suspended machine is a box at rest, and the ordinary start resumes it.
#[tokio::test]
async fn a_suspended_machine_reads_stopped_and_start_resumes_it() {
    let state = Arc::new(Mutex::new("suspended"));
    let server = StandIn::start(fly_machine(state.clone(), 200, true)).await;
    let fly = client(&server, TOKEN, None);
    assert_eq!(
        fly.state(ORG, APP, VOL, "m1").await.unwrap(),
        MachineState::Stopped
    );
    fly.start(APP, "m1").await.unwrap();
    assert_eq!(*state.lock().unwrap(), "started");
    assert_eq!(
        server.lines(),
        [
            format!("GET /v1/apps/{APP}/volumes/{VOL}"),
            format!("GET /v1/apps/{APP}/machines/m1"),
            format!("GET /v1/apps/{APP}/machines/m1"),
            format!("POST /v1/apps/{APP}/machines/m1/start"),
            format!("GET /v1/apps/{APP}/machines/m1/wait?state=started&timeout=30"),
        ]
    );
}

#[tokio::test]
async fn exec_runs_the_argv_with_an_explicit_timeout_and_returns_what_it_did() {
    let server = StandIn::start(|r| match r.body["command"][1].as_str() {
        Some("ok") => Reply::json(
            200,
            json!({ "exit_code": 0, "exit_signal": 0, "stdout": "ssh-ed25519 AAAAC3 host\n", "stderr": "" }),
        ),
        Some("fails") => Reply::json(
            200,
            json!({ "exit_code": 1, "stderr": "cat: fails: No such file" }),
        ),
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
async fn list_reports_each_box_shape_and_leaves_out_what_is_not_a_box() {
    let ours = |id: &str| json!({ "id": id, "name": "fletch_data", "state": "created" });
    let machine = |id: &str, state: &str| json!({ "id": id, "state": state, "config": { "metadata": { "fletch_box": "x" } } });
    let server = StandIn::start(move |r| match r.path.as_str() {
        "/v1/apps?org_slug=acme" => Reply::json(
            200,
            json!({ "apps": [
                { "name": "fletch-box-whole" },
                { "name": "fletch-box-disk-only" },
                { "name": "fletch-box-empty" },
                { "name": "fletch-box-machine-only" },
                { "name": "fletch-box-unlabelled" },
                { "name": "fletch-box-foreign-volume" },
                { "name": "fletch-box-two-disks" },
                { "name": "fletch-box-gone-since" },
                { "name": "my-api" },
            ]}),
        ),
        "/v1/apps/fletch-box-whole/machines" => Reply::json(200, json!([machine("ma", "stopped")])),
        "/v1/apps/fletch-box-whole/volumes" => Reply::json(200, json!([ours("vol_a")])),
        "/v1/apps/fletch-box-disk-only/volumes" => Reply::json(
            200,
            json!([
                ours("vol_b"),
                { "id": "vol_old", "name": "fletch_data", "state": "pending_destroy" },
            ]),
        ),
        "/v1/apps/fletch-box-empty/volumes" => Reply::json(
            200,
            json!([{ "id": "vol_c", "name": "fletch_data", "state": "destroyed" }]),
        ),
        "/v1/apps/fletch-box-machine-only/machines" => Reply::json(
            200,
            json!([{ "id": "md", "state": "started", "config": null,
                     "incomplete_config": { "metadata": { "fletch_box": "d" } } }]),
        ),
        "/v1/apps/fletch-box-unlabelled/machines" => Reply::json(
            200,
            json!([{ "id": "me", "state": "started", "config": { "metadata": {} } }]),
        ),
        "/v1/apps/fletch-box-unlabelled/volumes" => Reply::json(200, json!([ours("vol_e")])),
        "/v1/apps/fletch-box-foreign-volume/volumes" => Reply::json(
            200,
            json!([{ "id": "vol_f", "name": "pgdata", "state": "created" }]),
        ),
        "/v1/apps/fletch-box-two-disks/machines" => {
            Reply::json(200, json!([machine("mg", "stopped")]))
        }
        "/v1/apps/fletch-box-two-disks/volumes" => {
            Reply::json(200, json!([ours("vol_g1"), ours("vol_g2")]))
        }
        p if p.starts_with("/v1/apps/fletch-box-gone-since/") => Reply::json(404, json!({})),
        p if p.ends_with("/machines") || p.ends_with("/volumes") => Reply::json(200, json!([])),
        _ => Reply::json(500, json!({})),
    })
    .await;
    let listed = client(&server, TOKEN, None).list().await.unwrap();
    let fly = |app: &str, volume: &str, machine: &str| Handle::Fly {
        org: ORG.into(),
        app: app.into(),
        volume: volume.into(),
        machine: machine.into(),
    };
    assert_eq!(
        listed,
        [
            Listed {
                handle: fly("fletch-box-whole", "vol_a", "ma"),
                state: MachineState::Stopped,
            },
            Listed {
                handle: fly("fletch-box-disk-only", "vol_b", ""),
                state: MachineState::MachineGone,
            },
            Listed {
                handle: fly("fletch-box-empty", "", ""),
                state: MachineState::Gone,
            },
            Listed {
                handle: fly("fletch-box-machine-only", "", "md"),
                state: MachineState::DiskGone,
            },
        ],
        "an unlabelled machine, a volume not a box's, two live volumes and an app without the \
         prefix are not ours"
    );
    // An app with two live volumes is shown under neither: a box that names
    // the second is never offered the app with the first's id on it, which
    // `same_box` would not match.
    assert!(
        !listed
            .iter()
            .any(|l| matches!(&l.handle, Handle::Fly { app, .. } if app == "fletch-box-two-disks")),
        "{listed:?}"
    );
    assert!(
        !server.lines().iter().any(|l| l.contains("my-api")),
        "an app without the prefix is never even read"
    );
    // A box whose machine was lost still finds its disk in the listing.
    let stored = fly("fletch-box-disk-only", "vol_b", "m-lost");
    assert!(listed.iter().any(|l| l.handle.same_box(&stored)));
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
        match (r.method.as_str(), r.path.as_str()) {
            ("POST", p) if p.ends_with("/machines") => Reply::json(400, echo),
            ("GET", p) if p.ends_with("/volumes") => Reply::json(
                200,
                json!([{ "id": VOL, "name": "fletch_data", "state": "created" }]),
            ),
            ("DELETE", _) => {
                Reply::json(500, json!({ "error": "fm2_macaroonB9876543210 expired" }))
            }
            ("GET", p) if p.starts_with("/v1/apps?") => Reply::json(401, echo),
            ("POST", p) if p.ends_with("/exec") => Reply::json(412, echo),
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
