//! The per-agent `CODEX_HOME` a codex launch runs in: the host assembles it
//! (the agent's own sessions, the shared config linked in) and writes the
//! launch credential into it before every launch and turn, always through a
//! handle that never follows a link the agent planted.

use std::path::{Path, PathBuf};

use super::credential_file::{Entry, PrivateDir};
use super::host_login::codex::{self, Refresher};
use super::host_login::LoginError;
use crate::agent::accounts;
use crate::error::{Error, Result};

/// The per-agent `CODEX_HOME`, under the agent's own checkouts dir, the way
/// the container claude path keeps a per-agent `projects` dir
/// (`transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME`).
pub(crate) const OVERLAY_DIRNAME: &str = ".fletch-codex-home";

const AUTH_FILE: &str = "auth.json";

/// The overlay inside an agent's checkouts dir.
pub(crate) fn overlay_in(agent_dir: &Path) -> PathBuf {
    agent_dir.join(OVERLAY_DIRNAME)
}

/// The `CODEX_HOME` agent `agent_id` runs in. The one answer the launch, the
/// fork/rewind writer and the transcript locator all use, so a thread is
/// always where the next `codex exec resume` looks, whatever the agent's
/// checkout layout.
pub fn overlay_for_agent(agent_id: &str) -> Result<PathBuf> {
    Ok(overlay_in(&crate::workspace::agent_parent_dir(agent_id)?))
}

/// Open `overlay` for a host write: its parent (the agent's checkouts dir,
/// which the agent can write into but not replace) by handle, and the overlay
/// beneath it without following a link, recreated as a real directory when
/// the agent swapped it for anything else. Every host write into an overlay
/// goes through the handle this returns, so nothing the agent plants
/// redirects it.
pub(crate) fn open_overlay(overlay: &Path) -> Result<PrivateDir> {
    let (parent, name) = match (
        overlay.parent(),
        overlay.file_name().and_then(|n| n.to_str()),
    ) {
        (Some(parent), Some(name)) => (parent, name),
        _ => return Err(Error::Other("the codex overlay has no parent".into())),
    };
    std::fs::create_dir_all(parent)?;
    Ok(PrivateDir::open(parent)?.subdir(name)?)
}

/// Create (or repair) the overlay: its `sessions` dir, and the shared config
/// linked in from the user's codex home. The overlay belongs to Fletch, so
/// anything the agent left where a shared link belongs is replaced by the
/// link again.
pub fn prepare_overlay(overlay: &Path, home: &Path) -> Result<()> {
    let dir = open_overlay(overlay)?;
    dir.subdir("sessions")?;
    let source = accounts::shared_source_dir("codex", home);
    for item in accounts::shared_items("codex") {
        let target = source.join(item);
        match dir.entry(item)? {
            Entry::Link(at) if at == target => continue,
            Entry::Missing => {}
            _ => dir.remove(item)?,
        }
        if target.exists() {
            dir.symlink(&target, item)?;
        }
    }
    Ok(())
}

/// Write the launch credential for a codex run under the login in
/// `source_home` into `overlay` (see `host_login::codex::launch_file`), or
/// remove a stale one when nothing is stored or the login was refused.
pub(crate) fn write_launch_credential(
    source_home: &Path,
    overlay: &Path,
    refresh: Refresher<'_>,
    now: i64,
) -> Result<()> {
    let launch = codex::launch_file(source_home, refresh, now);
    let dir = open_overlay(overlay)?;
    match launch {
        Ok(Some(launch)) => {
            let text = serde_json::to_string_pretty(&launch)?;
            dir.write_file(AUTH_FILE, text.as_bytes(), 0o600)?;
            Ok(())
        }
        Ok(None) => Ok(dir.remove(AUTH_FILE)?),
        Err(LoginError::Revoked) => {
            dir.remove(AUTH_FILE)?;
            Err(Error::Other(codex::SIGNED_OUT_MSG.into()))
        }
        Err(LoginError::Unavailable(reason)) => Err(Error::Other(format!(
            "Couldn't refresh the Codex login ({reason}); its access token has expired."
        ))),
        Err(LoginError::SignedOut) => unreachable!("launch_file maps it to None"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::host_login::codex::{launch_credential, TokenResponse};
    use crate::agent::host_login::RefreshFailure;
    use base64::Engine as _;
    use parking_lot::Mutex;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    const NOW: i64 = 1_791_448_171;
    const DAY: i64 = 24 * 3600;

    fn jwt(iat: i64, exp: i64) -> String {
        let enc = |v: Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string().as_bytes())
        };
        format!(
            "{}.{}.sig",
            enc(json!({"alg": "RS256"})),
            enc(json!({"iat": iat, "exp": exp}))
        )
    }

    fn login(access_exp: i64) -> Value {
        json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": "id.jwt.value",
                "access_token": jwt(access_exp - 10 * DAY, access_exp),
                "refresh_token": "rt.original",
                "account_id": "acct-1",
                "future_token_field": 7
            },
            "last_refresh": "2026-10-02T15:37:10.326659Z",
            "future_top_level": {"kept": true}
        })
    }

    fn write_login(dir: &Path, auth: &Value) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(AUTH_FILE), auth.to_string()).unwrap();
    }

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn never(_: &str) -> std::result::Result<TokenResponse, RefreshFailure> {
        panic!("no refresh expected")
    }

    fn rotated() -> TokenResponse {
        TokenResponse {
            access_token: Some(jwt(NOW, NOW + 10 * DAY)),
            id_token: Some("id.new".into()),
            refresh_token: Some("rt.rotated".into()),
        }
    }

    /// A source home and an overlay inside an agent dir, in a fresh tempdir.
    fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let source = td.path().join("src");
        let overlay = overlay_in(&td.path().join("agent"));
        (td, source, overlay)
    }

    /// Makes the source home unwritable (so a save fails) until dropped.
    #[cfg(unix)]
    struct ReadOnly<'a>(&'a Path);

    #[cfg(unix)]
    impl<'a> ReadOnly<'a> {
        fn set(dir: &'a Path) -> Self {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
            Self(dir)
        }
    }

    #[cfg(unix)]
    impl Drop for ReadOnly<'_> {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn a_fresh_login_is_copied_into_the_overlay_without_its_refresh_token() {
        let (_td, source, overlay) = dirs();
        let auth = login(NOW + 5 * DAY);
        write_login(&source, &auth);

        write_launch_credential(&source, &overlay, &never, NOW).unwrap();

        let written = read(&overlay.join(AUTH_FILE));
        assert_eq!(written, launch_credential(&auth));
        assert_eq!(read(&source.join(AUTH_FILE)), auth);
    }

    #[test]
    fn a_near_expiry_login_is_refreshed_on_the_host_and_the_rotation_persisted() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        let seen = Mutex::new(String::new());
        let refresh = |rt: &str| {
            *seen.lock() = rt.to_string();
            Ok(rotated())
        };

        write_launch_credential(&source, &overlay, &refresh, NOW).unwrap();

        assert_eq!(*seen.lock(), "rt.original");
        let host = read(&source.join(AUTH_FILE));
        assert_eq!(host["tokens"]["refresh_token"], "rt.rotated");
        assert_eq!(host["tokens"]["id_token"], "id.new");
        assert_eq!(host["tokens"]["future_token_field"], 7);
        assert_eq!(host["future_top_level"], json!({"kept": true}));
        assert_eq!(host["last_refresh"], "2026-10-08T08:29:31.000000Z");
        let launch = read(&overlay.join(AUTH_FILE));
        assert_eq!(launch["tokens"]["refresh_token"], "");
        assert_eq!(
            launch["tokens"]["access_token"],
            host["tokens"]["access_token"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn credential_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        write_launch_credential(&source, &overlay, &|_| Ok(rotated()), NOW).unwrap();
        for path in [source.join(AUTH_FILE), overlay.join(AUTH_FILE)] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{}", path.display());
        }
    }

    #[test]
    fn concurrent_launches_under_one_login_refresh_once() {
        let td = tempfile::tempdir().unwrap();
        let source = td.path().join("src");
        write_login(&source, &login(NOW + 3600));
        let calls = AtomicUsize::new(0);
        let refresh = |_: &str| {
            calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            Ok(rotated())
        };
        std::thread::scope(|s| {
            for i in 0..4 {
                let overlay = overlay_in(&td.path().join(format!("agent{i}")));
                let (source, refresh) = (&source, &refresh);
                s.spawn(move || write_launch_credential(source, &overlay, refresh, NOW).unwrap());
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_revoked_refresh_fails_the_launch_and_withdraws_the_overlay_credential() {
        accounts::with_test_root(|root| {
            let source = root.join("codex/work");
            let overlay = overlay_in(&root.join("agent"));
            write_login(&source, &login(NOW - 1));
            open_overlay(&overlay).unwrap();
            std::fs::write(overlay.join(AUTH_FILE), "stale").unwrap();

            let err =
                write_launch_credential(&source, &overlay, &|_| Err(RefreshFailure::Rejected), NOW)
                    .unwrap_err();

            assert!(err.to_string().contains("Sign in again"), "{err}");
            assert!(!overlay.join(AUTH_FILE).exists());
        });
    }

    #[test]
    fn a_revoked_login_reads_signed_out_until_a_new_sign_in() {
        accounts::with_test_root(|root| {
            let source = root.join("codex/work");
            let overlay = overlay_in(&root.join("agent"));
            write_login(&source, &login(NOW - 1));
            let probe = || super::super::auth_probe::probe_dir("codex", &source).status;
            assert_eq!(probe(), super::super::AuthStatus::SignedIn);

            let _ =
                write_launch_credential(&source, &overlay, &|_| Err(RefreshFailure::Rejected), NOW);
            assert_eq!(probe(), super::super::AuthStatus::SignedOut);

            let mut relogged = login(NOW + 5 * DAY);
            relogged["tokens"]["refresh_token"] = Value::String("rt.new-login".into());
            write_login(&source, &relogged);
            assert_eq!(probe(), super::super::AuthStatus::SignedIn);
        });
    }

    #[test]
    fn a_refusal_after_a_concurrent_rotation_retries_with_the_new_token() {
        accounts::with_test_root(|root| {
            let source = root.join("codex/work");
            let overlay = overlay_in(&root.join("agent"));
            write_login(&source, &login(NOW + 3600));
            let sent = Mutex::new(Vec::new());
            let refresh = |rt: &str| {
                sent.lock().push(rt.to_string());
                if rt == "rt.original" {
                    let mut moved = login(NOW + 3600);
                    moved["tokens"]["refresh_token"] = Value::String("rt.elsewhere".into());
                    write_login(&source, &moved);
                    Err(RefreshFailure::Rejected)
                } else {
                    Ok(rotated())
                }
            };

            write_launch_credential(&source, &overlay, &refresh, NOW).unwrap();

            assert_eq!(*sent.lock(), vec!["rt.original", "rt.elsewhere"]);
            assert_eq!(
                read(&source.join(AUTH_FILE))["tokens"]["refresh_token"],
                "rt.rotated"
            );
            assert!(!codex::is_revoked(&source));
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_rotation_that_cannot_be_saved_is_kept_and_used_by_the_next_launch() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        {
            let _ro = ReadOnly::set(&source);
            write_launch_credential(&source, &overlay, &|_| Ok(rotated()), NOW).unwrap();
            assert_eq!(
                read(&source.join(AUTH_FILE))["tokens"]["refresh_token"],
                "rt.original"
            );

            write_launch_credential(&source, &overlay, &never, NOW).unwrap();
        }

        let launch = read(&overlay.join(AUTH_FILE));
        assert_eq!(
            launch["tokens"]["access_token"],
            json!(jwt(NOW, NOW + 10 * DAY))
        );
        assert_eq!(launch["tokens"]["refresh_token"], "");
    }

    #[cfg(unix)]
    #[test]
    fn a_kept_rotation_is_saved_once_the_store_takes_it() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        {
            let _ro = ReadOnly::set(&source);
            write_launch_credential(&source, &overlay, &|_| Ok(rotated()), NOW).unwrap();
        }

        write_launch_credential(&source, &overlay, &never, NOW).unwrap();

        assert_eq!(
            read(&source.join(AUTH_FILE))["tokens"]["refresh_token"],
            "rt.rotated"
        );
        write_launch_credential(&source, &overlay, &never, NOW).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_sign_in_after_a_failed_save_wins_over_the_kept_rotation() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        {
            let _ro = ReadOnly::set(&source);
            write_launch_credential(&source, &overlay, &|_| Ok(rotated()), NOW).unwrap();
        }
        let mut signed_in = login(NOW + 9 * DAY);
        signed_in["tokens"]["refresh_token"] = Value::String("rt.new-sign-in".into());
        signed_in["tokens"]["account_id"] = Value::String("acct-after-sign-in".into());
        write_login(&source, &signed_in);

        write_launch_credential(&source, &overlay, &never, NOW).unwrap();

        assert_eq!(read(&source.join(AUTH_FILE)), signed_in);
        assert_eq!(
            read(&overlay.join(AUTH_FILE))["tokens"]["account_id"],
            "acct-after-sign-in"
        );
    }

    #[test]
    fn a_transient_failure_still_launches_while_the_token_is_valid() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW + 3600));
        let offline = |_: &str| Err(RefreshFailure::Failed("offline".into()));

        write_launch_credential(&source, &overlay, &offline, NOW).unwrap();

        assert_eq!(
            read(&overlay.join(AUTH_FILE))["tokens"]["refresh_token"],
            ""
        );
    }

    #[test]
    fn a_transient_failure_with_an_expired_token_fails_the_turn() {
        let (_td, source, overlay) = dirs();
        write_login(&source, &login(NOW - 1));
        let offline = |_: &str| Err(RefreshFailure::Failed("offline".into()));

        let err = write_launch_credential(&source, &overlay, &offline, NOW).unwrap_err();

        assert!(err.to_string().contains("offline"), "{err}");
    }

    #[test]
    fn no_host_login_leaves_no_credential_in_the_overlay() {
        let (_td, source, overlay) = dirs();
        open_overlay(&overlay).unwrap();
        std::fs::write(overlay.join(AUTH_FILE), "{}").unwrap();
        write_launch_credential(&source, &overlay, &never, NOW).unwrap();
        assert!(!overlay.join(AUTH_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_turn_after_the_overlay_was_swapped_for_a_link_never_writes_through_it() {
        let (td, source, overlay) = dirs();
        let auth = login(NOW + 5 * DAY);
        write_login(&source, &auth);
        write_launch_credential(&source, &overlay, &never, NOW).unwrap();
        std::fs::rename(&overlay, td.path().join("moved")).unwrap();
        std::os::unix::fs::symlink(&source, &overlay).unwrap();

        write_launch_credential(&source, &overlay, &never, NOW).unwrap();

        assert_eq!(read(&source.join(AUTH_FILE)), auth);
        assert!(overlay.symlink_metadata().unwrap().is_dir());
        assert_eq!(
            read(&overlay.join(AUTH_FILE))["tokens"]["refresh_token"],
            ""
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_turn_replaces_a_planted_credential_link_instead_of_writing_through_it() {
        let (_td, source, overlay) = dirs();
        let auth = login(NOW + 5 * DAY);
        write_login(&source, &auth);
        open_overlay(&overlay).unwrap();
        std::os::unix::fs::symlink(source.join(AUTH_FILE), overlay.join(AUTH_FILE)).unwrap();

        write_launch_credential(&source, &overlay, &never, NOW).unwrap();

        assert_eq!(read(&source.join(AUTH_FILE)), auth);
        assert!(!overlay
            .join(AUTH_FILE)
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn the_overlay_relinks_shared_config_and_replaces_what_the_agent_put_there() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path().join("home");
        let shared = home.join(".codex");
        std::fs::create_dir_all(shared.join("skills")).unwrap();
        std::fs::write(shared.join("config.toml"), "model = \"x\"").unwrap();
        let overlay = overlay_in(&td.path().join("root"));
        std::fs::create_dir_all(&overlay).unwrap();
        std::fs::write(overlay.join("config.toml"), "[mcp_servers.evil]").unwrap();

        prepare_overlay(&overlay, &home).unwrap();

        assert!(overlay.join("sessions").is_dir());
        for item in ["config.toml", "skills"] {
            assert_eq!(
                std::fs::read_link(overlay.join(item)).unwrap(),
                shared.join(item)
            );
        }
        assert!(overlay.join("AGENTS.md").symlink_metadata().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn an_overlay_swapped_for_a_symlink_is_recreated_as_a_directory() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let elsewhere = td.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let overlay = overlay_in(td.path());
        std::os::unix::fs::symlink(&elsewhere, &overlay).unwrap();

        prepare_overlay(&overlay, &home).unwrap();

        assert!(overlay.symlink_metadata().unwrap().file_type().is_dir());
        assert!(!elsewhere.join("sessions").exists());
    }
}
