//! The stored connection and the provider-neutral types: a token is stored
//! only once the provider accepts it, disconnecting forgets it, the status
//! Settings shows is read from storage alone, and a handle survives the trip
//! through its string.

use serde_json::json;

use super::*;
use crate::cloud::stand_in::{Reply, StandIn};

fn db() -> (tempfile::TempDir, crate::DbState) {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::database::init(dir.path()).unwrap();
    (dir, db)
}

fn fly_at(server: &StandIn, pasted: &Connection) -> Client {
    Client::Fly(
        fly::Client::new(pasted, server.url())
            .unwrap()
            .with_pace(Duration::from_millis(1)),
    )
}

#[tokio::test]
async fn connect_stores_the_token_only_once_the_provider_accepts_it() {
    let (_dir, db) = db();

    let refusing = StandIn::start(|_| Reply::json(401, json!({ "error": "unauthorized" }))).await;
    let pasted = Connection::new(ProviderId::Fly, "bad-token-0123456789", Some("acme")).unwrap();
    let err = connect_with(&db, pasted.clone(), &fly_at(&refusing, &pasted))
        .await
        .unwrap_err();
    assert!(
        err.to_string().starts_with("Fly.io refused the token"),
        "{err}"
    );
    assert!(connection(&db.lock(), ProviderId::Fly).unwrap().is_none());
    assert_eq!(
        crate::database::get_setting(&db.lock(), "cloud.fly.org"),
        None,
        "a refused paste stores nothing at all"
    );

    let accepting = StandIn::start(|_| Reply::json(200, json!({ "apps": [] }))).await;
    let pasted = Connection::new(ProviderId::Fly, "  good-token-0123456789\n", Some("  ")).unwrap();
    let status = connect_with(&db, pasted.clone(), &fly_at(&accepting, &pasted))
        .await
        .unwrap();
    assert_eq!(
        status,
        CloudProviderStatus {
            id: "fly",
            label: "Fly.io",
            connected: true,
            account: Some("personal".into()),
        },
        "a blank org is the personal one"
    );
    assert_eq!(accepting.lines(), ["GET /v1/apps?org_slug=personal"]);
    assert_eq!(
        accepting.requests()[0].header("authorization"),
        Some("Bearer good-token-0123456789"),
        "the token is trimmed before it is sent or stored"
    );
    let stored = connection(&db.lock(), ProviderId::Fly).unwrap().unwrap();
    assert_eq!(stored.token(), "good-token-0123456789");
    assert_eq!(stored.account(), "personal");
    assert_eq!(stored.region(), None);
}

#[tokio::test]
async fn disconnect_forgets_the_token_and_the_org_but_not_the_region() {
    let (_dir, db) = db();
    let accepting = StandIn::start(|_| Reply::json(200, json!({ "apps": [] }))).await;
    let pasted = Connection::new(ProviderId::Fly, "token-0123456789", Some("acme")).unwrap();
    connect_with(&db, pasted.clone(), &fly_at(&accepting, &pasted))
        .await
        .unwrap();
    crate::database::set_setting(&db.lock(), "cloud.fly.region", "ams").unwrap();
    assert_eq!(
        connection(&db.lock(), ProviderId::Fly)
            .unwrap()
            .unwrap()
            .region(),
        Some("ams")
    );

    disconnect(&db.lock(), ProviderId::Fly).unwrap();
    let conn = db.lock();
    assert!(connection(&conn, ProviderId::Fly).unwrap().is_none());
    assert_eq!(crate::secrets::get(&conn, "cloud.fly.token").unwrap(), None);
    assert_eq!(
        crate::database::get_setting(&conn, "cloud.fly.org").as_deref(),
        Some("")
    );
    assert_eq!(
        crate::database::get_setting(&conn, "cloud.fly.region").as_deref(),
        Some("ams")
    );
}

/// `statuses` takes a database connection and nothing else — no client, no
/// address — so it cannot reach the network; here no server exists at all.
#[test]
fn statuses_are_read_from_storage_in_the_settings_shape() {
    let (_dir, db) = db();
    let conn = db.lock();
    assert_eq!(
        serde_json::to_value(statuses(&conn).unwrap()).unwrap(),
        json!([{ "id": "fly", "label": "Fly.io", "connected": false, "account": null }])
    );

    crate::secrets::set(&conn, "cloud.fly.token", "token-0123456789").unwrap();
    assert_eq!(
        serde_json::to_value(statuses(&conn).unwrap()).unwrap(),
        json!([{ "id": "fly", "label": "Fly.io", "connected": true, "account": "personal" }]),
        "a token stored without an org acts in the personal one"
    );
    crate::database::set_setting(&conn, "cloud.fly.org", "acme").unwrap();
    assert_eq!(
        serde_json::to_value(statuses(&conn).unwrap()).unwrap(),
        json!([{ "id": "fly", "label": "Fly.io", "connected": true, "account": "acme" }])
    );
}

#[test]
fn a_blank_token_is_refused_and_debug_never_shows_one() {
    let err = Connection::new(ProviderId::Fly, " \n", None).unwrap_err();
    assert_eq!(err.to_string(), "paste a Fly.io API token to connect");

    let c = Connection::new(ProviderId::Fly, "FlyV1 fm2_secret0123456789", Some("acme"))
        .unwrap()
        .with_region(Some(" "));
    assert_eq!(c.region(), None, "a blank region lets the provider choose");
    let debug = format!("{c:?}");
    assert!(!debug.contains("fm2_secret"), "{debug}");
    assert!(debug.contains("acme"), "{debug}");
}

#[test]
fn provider_ids_round_trip_and_nothing_else_parses() {
    for &p in ProviderId::ALL {
        assert_eq!(ProviderId::parse(p.as_str()), Some(p));
    }
    assert_eq!(ProviderId::Fly.as_str(), "fly");
    assert_eq!(ProviderId::Fly.label(), "Fly.io");
    for other in ["Fly", "fly.io", "", "daytona"] {
        assert_eq!(ProviderId::parse(other), None, "{other:?}");
    }
}

#[test]
fn a_handle_survives_its_string_and_a_malformed_one_is_refused() {
    let handle = Handle::Fly {
        app: "fletch-box-ladakh-otter-k3j9x2ab".into(),
        machine: "3d8d9dd6b26e89".into(),
    };
    let stored = handle.to_string();
    assert_eq!(
        stored,
        "fly:fletch-box-ladakh-otter-k3j9x2ab/3d8d9dd6b26e89"
    );
    assert_eq!(Handle::parse(&stored).unwrap(), handle);
    assert_eq!(handle.provider(), ProviderId::Fly);
    assert_eq!(
        handle.endpoint(),
        Endpoint::Tls {
            host: "fletch-box-ladakh-otter-k3j9x2ab.fly.dev".into(),
            port: 443
        }
    );

    let machineless = Handle::Fly {
        app: "fletch-box-a".into(),
        machine: String::new(),
    };
    assert_eq!(
        Handle::parse(&machineless.to_string()).unwrap(),
        machineless
    );

    for bad in [
        "",
        "fly",
        "fly:app",
        "fly:/m1",
        "fly:App/m1",
        "fly:app/m 1",
        "fly:app/../m1",
        "gcp:app/m1",
    ] {
        assert!(Handle::parse(bad).is_err(), "{bad:?}");
    }
}
