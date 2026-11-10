//! Host-side commands for the paired-device remote server (Settings →
//! Devices). These are not remote ops: they are how the *desktop* turns the
//! listener on, mints pairing codes, revokes devices and lists the other hosts
//! nearby.

use std::sync::Arc;
use std::time::Duration;
use tauri::State;

use crate::database;
use crate::error::{Error, Result};
use crate::remote::{NearbyHost, PairingInvite, RemoteState, RemoteStatus};
use crate::DbState;

#[tauri::command]
pub fn remote_status(remote: State<'_, Arc<RemoteState>>) -> RemoteStatus {
    remote.status()
}

/// Start or stop the listener and persist the intent, so the next launch agrees
/// with the toggle. Returns the resulting status: the UI needs the bound port
/// and the address list the moment the listener comes up, and a second
/// round-trip could observe a different state.
#[tauri::command]
pub async fn remote_set_enabled(
    remote: State<'_, Arc<RemoteState>>,
    db: State<'_, DbState>,
    enabled: bool,
) -> Result<RemoteStatus> {
    let (port, relay) = {
        let conn = db.lock();
        (
            crate::remote::parse_port(
                database::get_setting(&conn, crate::remote::PORT_SETTING).as_deref(),
            ),
            // The relay comes with remote access on the desktop: with nothing
            // stored, Fletch's own is written in (see `ensure_relay_setting`).
            enabled.then(|| crate::remote::ensure_relay_setting(&conn)),
        )
    };
    if enabled {
        // Set before `start`, so the link comes up with the listener. Left
        // alone when it already matches, which spares a live link a reconnect.
        if let Some(relay) = relay.filter(|url| remote.relay_url().as_ref() != Some(url)) {
            remote.inner().set_relay(Some(relay))?;
        }
        remote.inner().start(port)?;
    } else {
        remote.stop();
    }
    // Persisted after the bind so a port that cannot be opened does not leave
    // the app trying (and failing) to listen on every future launch.
    {
        let conn = db.lock();
        database::set_setting(
            &conn,
            crate::remote::ENABLED_SETTING,
            if enabled { "true" } else { "false" },
        )?;
    }
    Ok(remote.status())
}

/// Change the listen port. Applied live first (`RemoteState::set_port`: a
/// running listener moves, an idle one just records it) and persisted only
/// once that succeeded, so a port that cannot be bound is refused with the old
/// listener still up and nothing stored — the same order as `remote_set_enabled`.
#[tauri::command]
pub async fn remote_set_port(
    remote: State<'_, Arc<RemoteState>>,
    db: State<'_, DbState>,
    port: u16,
) -> Result<RemoteStatus> {
    if port == 0 {
        return Err(Error::Other("Choose a port between 1 and 65535.".into()));
    }
    remote.inner().set_port(port)?;
    {
        let conn = db.lock();
        database::set_setting(&conn, crate::remote::PORT_SETTING, &port.to_string())?;
    }
    Ok(remote.status())
}

/// Set or clear the relay base URL: the host link comes up (or goes away)
/// immediately, and the setting is persisted so the next launch agrees.
///
/// Persisted only after `set_relay` accepted the URL, for the same reason
/// `remote_set_enabled` persists after the bind: a value the link could never
/// use should not survive the restart. Clearing is stored as an empty string,
/// which is what "no relay" reads as at launch.
#[tauri::command]
pub async fn remote_set_relay(
    remote: State<'_, Arc<RemoteState>>,
    db: State<'_, DbState>,
    url: Option<String>,
) -> Result<RemoteStatus> {
    remote.inner().set_relay(url.clone())?;
    {
        let conn = db.lock();
        database::set_setting(
            &conn,
            crate::remote::RELAY_URL_SETTING,
            url.as_deref().unwrap_or("").trim(),
        )?;
    }
    Ok(remote.status())
}

/// Mint a single-use pairing code and the `fletch://pair` deep link that
/// carries it. Refuses while the listener is down — a code nothing can be typed
/// into is worse than an error — and while the device store is unwritable,
/// since that pairing would stop working at the next launch.
///
/// `preset` is the access the code grants (`full` or `control`); absent means
/// `full`, which is what every pairing granted before presets existed.
#[tauri::command]
pub fn remote_begin_pairing(
    remote: State<'_, Arc<RemoteState>>,
    preset: Option<String>,
) -> Result<PairingInvite> {
    let status = remote.status();
    if let Some(error) = status.error {
        return Err(Error::Other(error));
    }
    if !status.listening {
        return Err(Error::Other(
            "Turn on mobile access before pairing a device.".into(),
        ));
    }
    remote.begin_pairing(preset.as_deref())
}

/// The answer to a device asking to pair (the `remote:pair-request` prompt).
/// `false` when that prompt is already over — answered in another window,
/// withdrawn by the phone, or timed out — which the sheet treats as done.
#[tauri::command]
pub fn remote_answer_pair_request(
    remote: State<'_, Arc<RemoteState>>,
    id: String,
    accept: bool,
) -> bool {
    remote.prompts().answer(&id, accept)
}

/// The longest one browse may run. Settings asks for a couple of seconds at a
/// time; the bound only stops a stray caller parking a blocking thread.
const MAX_BROWSE: Duration = Duration::from_secs(10);

/// The other Fletch hosts announcing themselves on this network
/// (docs/remote-protocol.md, "Discovery"), for Settings › Devices › Hosts:
/// browse for `duration_ms` and answer with every host seen by the end, this
/// one left out. Nothing browses between calls, so the list is only gathered
/// while that tab is open and asking. Whether or not remote access is on: a
/// Mac pairing *with* another host needs no listener of its own.
#[tauri::command]
pub async fn remote_browse_nearby(
    remote: State<'_, Arc<RemoteState>>,
    duration_ms: u64,
) -> Result<Vec<NearbyHost>> {
    let own_id = remote.host_id();
    let duration = Duration::from_millis(duration_ms).min(MAX_BROWSE);
    tauri::async_runtime::spawn_blocking(move || crate::remote::browse_nearby(duration, &own_id))
        .await
        .map_err(|e| Error::Other(e.to_string()))?
        .map_err(Error::Other)
}

#[tauri::command]
pub fn remote_revoke_device(
    remote: State<'_, Arc<RemoteState>>,
    device_id: String,
) -> Result<RemoteStatus> {
    remote.revoke_device(&device_id)?;
    Ok(remote.status())
}
