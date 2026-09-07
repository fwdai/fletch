//! Host-side commands for the paired-device remote server (Settings → Mobile
//! devices). These are not remote ops: they are how the *desktop* turns the
//! listener on, mints pairing codes and revokes devices.

use std::sync::Arc;
use tauri::State;

use crate::database;
use crate::error::{Error, Result};
use crate::remote::{PairingInvite, RemoteState, RemoteStatus};
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
    let port = {
        let conn = db.lock();
        crate::remote::parse_port(
            database::get_setting(&conn, crate::remote::PORT_SETTING).as_deref(),
        )
    };
    if enabled {
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

/// Mint a single-use pairing code and the `fletch://pair` deep link that
/// carries it. Refuses while the listener is down — a code nothing can be typed
/// into is worse than an error — and while the device store is unwritable,
/// since that pairing would stop working at the next launch.
#[tauri::command]
pub fn remote_begin_pairing(remote: State<'_, Arc<RemoteState>>) -> Result<PairingInvite> {
    let status = remote.status();
    if let Some(error) = status.error {
        return Err(Error::Other(error));
    }
    if !status.listening {
        return Err(Error::Other(
            "Turn on mobile access before pairing a device.".into(),
        ));
    }
    Ok(remote.begin_pairing())
}

#[tauri::command]
pub fn remote_revoke_device(
    remote: State<'_, Arc<RemoteState>>,
    device_id: String,
) -> Result<RemoteStatus> {
    remote.revoke_device(&device_id)?;
    Ok(remote.status())
}
