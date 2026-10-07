//! Provider sign-in PTY: open, write stdin, resize, and close.
//!
//! The command each one runs is pinned in [`crate::provider_login`]; the
//! renderer only ever names a provider.

use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{Error, Result};
use crate::provider_login::{login_args, ProviderLoginSessions};
use crate::pty_session::{serialize_bytes_b64, PtySession, PtySpawn};

#[derive(Clone, serde::Serialize)]
struct LoginOutputPayload {
    id: String,
    #[serde(serialize_with = "serialize_bytes_b64")]
    bytes: Vec<u8>,
}

#[derive(Clone, serde::Serialize)]
struct LoginExitPayload {
    id: String,
    success: bool,
    message: String,
}

/// The key a sign-in runs under, and the `id` its events carry: the provider
/// alone for its default account, `<provider>:<account>` for a Fletch-managed
/// one. The frontend builds the same string (`loginKey` in
/// SettingsScreen/ProviderLogin/loginSessions.ts).
fn session_key(id: &str, account: Option<&str>) -> String {
    match account.filter(|a| !crate::agent::accounts::is_default(a)) {
        Some(account) => format!("{id}:{account}"),
        None => id.to_string(),
    }
}

/// Run a provider's pinned sign-in command under a PTY so the user can complete
/// it inside Settings. Output streams as `provider-login:output` (raw bytes,
/// base64) and the end of the flow as `provider-login:exit`, both carrying the
/// [`session_key`] as their `id`.
///
/// `account` names a Fletch-managed account directory (see `agent::accounts`)
/// to sign in: the CLI runs with its config-dir env pointed there, so the
/// login lands in that account and not the user's own. Absent or the default
/// signs the CLI's own directory in, as before.
///
/// Idempotent: while a sign-in under the same key is live this does nothing,
/// so re-opening the row re-attaches to the flow already in progress rather
/// than starting a second one. Errors when the provider has no login command
/// or its binary can't be resolved.
#[tauri::command]
pub fn open_provider_login(
    app: AppHandle,
    sessions: State<'_, ProviderLoginSessions>,
    id: String,
    account: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<()> {
    let args =
        login_args(&id).ok_or_else(|| Error::Other(format!("`{id}` has no in-app sign-in.")))?;
    let (bin, label) = crate::agent::provider_bin_label(&id)
        .ok_or_else(|| Error::Other(format!("unknown provider `{id}`")))?;
    let home = dirs::home_dir()
        .ok_or_else(|| Error::Other("Could not determine your home directory.".into()))?;
    // Honours a custom binary path override, so the CLI that gets signed in is
    // the same one the app will spawn for turns.
    let program = crate::agent::resolve_agent_bin(&id, bin, label, &home)?;
    let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();

    // A managed account's directory is (re)made right before the CLI writes
    // into it, so its shared config links are current for the login too.
    let env: Vec<(String, String)> = match account
        .as_deref()
        .filter(|a| !crate::agent::accounts::is_default(a))
    {
        Some(acct) => {
            crate::agent::accounts::ensure_account_dir(&id, acct)?;
            crate::agent::accounts::account_env(&id, acct)?
        }
        None => Vec::new(),
    };

    let key = session_key(&id, account.as_deref());
    let app_out = app.clone();
    let id_out = key.clone();
    let id_exit = key.clone();

    // Held across the spawn so a login that dies immediately (unusable binary,
    // instant refusal) can't have its `on_exit` removal race ahead of the
    // insert below and leave a dead session parked in the map.
    let mut live = sessions.lock();
    if live.contains_key(&key) {
        return Ok(());
    }
    let session = PtySession::spawn(
        PtySpawn {
            program: std::path::Path::new(&program),
            args: &args,
            cwd: &home,
            // `PtySession` already layers the user's login-shell environment
            // (plus TERM) over the inherited one — which is what these flows
            // need for PATH, HOME and BROWSER. Only the account's config-dir
            // var is added on top.
            env: &env,
            cols,
            rows,
            kill_plan: crate::sandbox::KillHandle::ProcessGroup,
        },
        move |bytes| {
            let _ = app_out.emit(
                "provider-login:output",
                LoginOutputPayload {
                    id: id_out.clone(),
                    bytes,
                },
            );
        },
        move |exit| {
            tracing::info!(
                success = exit.success,
                message = %exit.message,
                provider = %id_exit,
                "provider sign-in exited"
            );
            if let Some(sessions) = app.try_state::<ProviderLoginSessions>() {
                sessions.lock().remove(&id_exit);
            }
            let _ = app.emit(
                "provider-login:exit",
                LoginExitPayload {
                    id: id_exit.clone(),
                    success: exit.success,
                    message: exit.message,
                },
            );
        },
    )?;
    live.insert(key, session);
    Ok(())
}

/// Write bytes to a live sign-in PTY's stdin. `id` is the [`session_key`].
#[tauri::command]
pub fn write_provider_login(
    sessions: State<'_, ProviderLoginSessions>,
    id: String,
    data: String,
) -> Result<()> {
    sessions
        .lock()
        .get(&id)
        .ok_or_else(|| Error::Other(format!("no sign-in running for `{id}`")))?
        .write(data.as_bytes())
}

/// Resize a live sign-in PTY to match its terminal.
#[tauri::command]
pub fn resize_provider_login(
    sessions: State<'_, ProviderLoginSessions>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<()> {
    sessions
        .lock()
        .get(&id)
        .ok_or_else(|| Error::Other(format!("no sign-in running for `{id}`")))?
        .resize(cols, rows)
}

/// Kill a provider's sign-in PTY. Idempotent: does nothing when none is
/// running. Unmounting the terminal deliberately does *not* call this — a
/// collapsed row must not abort a login in progress.
#[tauri::command]
pub fn close_provider_login(sessions: State<'_, ProviderLoginSessions>, id: String) -> Result<()> {
    sessions.lock().remove(&id); // Drop impl kills the PTY
    Ok(())
}
