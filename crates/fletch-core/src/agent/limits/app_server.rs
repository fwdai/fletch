//! Codex limits on demand: one `account/rateLimits/read` over `codex
//! app-server`, which answers from the account's login without spending any
//! model quota.
//!
//! The app-server speaks JSON-RPC 2.0 over stdio, one JSON object per line (no
//! Content-Length framing): `initialize`, the `initialized` notification, then
//! the read. The child lives for exactly one exchange, bounded by [`TIMEOUT`],
//! and is killed — its whole process group, since an npm install runs codex
//! behind a node wrapper — whatever the outcome.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use super::{from_app_server, RefreshOutcome};
use crate::agent::accounts;
use crate::error::{Error, Result};

/// Community tools saw 4 s time out intermittently on a cold start.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// The app-server's untyped internal error, which is how a 401 from the
/// backend surfaces — but also how any other backend failure does, so the
/// message has to name the auth failure before it counts as signed out.
const INTERNAL_ERROR_CODE: i64 = -32603;

/// Whether an internal error's message is about the login rather than, say,
/// the network: "failed to fetch codex rate limits: 401" is the known form.
fn names_auth_failure(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("401") || m.contains("unauthori") || m.contains("logged in") || m.contains("login")
}

const INITIALIZE_ID: i64 = 1;
const READ_ID: i64 = 2;

/// Read the limits of the codex account whose home is `account_dir` (`None` =
/// the default account, the CLI's own home).
///
/// The app-server is a real codex, so it never runs in the account's own
/// home: it would refresh the login itself, outside the host's single-flight,
/// and could spend a refresh token the host is about to use. It runs in a
/// temporary `CODEX_HOME` ([`LimitsHome`]) holding the launch copy of the
/// login (refresh token blanked, refreshed by the host first if due) and the
/// shared config, removed after the read. A managed account also has the default account's
/// `OPENAI_API_KEY` removed, so it answers with its own login or none.
pub async fn read_limits(account_dir: Option<&Path>) -> Result<RefreshOutcome> {
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    let (bin, label) = crate::agent::provider_bin_label("codex")
        .ok_or_else(|| Error::Other("codex has no binary name".into()))?;
    let program = crate::agent::resolve_agent_bin("codex", bin, label, &home)?;
    let source = crate::agent::host_login::codex::source_home(account_dir, &home);
    let prepared = {
        let home = home.clone();
        tokio::task::spawn_blocking(move || {
            limits_home(
                &source,
                &home,
                &crate::agent::host_login::codex::http_refresh,
                now(),
            )
        })
        .await
        .map_err(|e| Error::Other(format!("preparing the limits read failed: {e}")))??
    };
    let Some(limits_home) = prepared else {
        return Ok(RefreshOutcome::SignedOut);
    };
    let codex_home = limits_home.path.clone();

    let mut cmd = tokio::process::Command::new(program);
    cmd.arg("app-server")
        .current_dir(&home)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::bin_resolve::apply_login_shell_env(cmd.as_std_mut());
    // After the login-shell layer, so neither its `CODEX_HOME` nor its key
    // can stand in for the account's own.
    cmd.env("CODEX_HOME", &codex_home);
    if account_dir.is_some() {
        for var in accounts::ambient_credential_vars("codex") {
            cmd.env_remove(var);
        }
    }

    #[cfg(unix)]
    let (mut child, pgid) = crate::pty_session::spawn_in_own_group(&mut cmd)?;
    #[cfg(not(unix))]
    let mut child = cmd.spawn()?;

    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let answer = match (stdin, stdout) {
        (Some(stdin), Some(stdout)) => {
            tokio::time::timeout(TIMEOUT, converse(BufReader::new(stdout), stdin, now()))
                .await
                .unwrap_or_else(|_| {
                    Err(Error::Other(format!(
                        "codex app-server did not answer within {}s",
                        TIMEOUT.as_secs()
                    )))
                })
        }
        _ => Err(Error::Other(
            "codex app-server pipes were not set up".into(),
        )),
    };

    // The server would otherwise wait on stdin forever; nothing it says after
    // the read matters.
    #[cfg(unix)]
    {
        let _ =
            tokio::task::spawn_blocking(move || crate::pty_session::kill_process_group(pgid)).await;
    }
    let _ = child.kill().await;
    drop(limits_home);
    answer
}

/// The dir every limits read's temporary home is made in: inside the codex
/// accounts dir, which every agent's seatbelt profile denies reading and
/// writing (`seatbelt::deny_host_codex_logins`). The host's temp dir would be
/// granted to agents, which could read the login there or swap a config link
/// before the unsandboxed app-server starts.
pub(crate) fn limits_parent() -> Result<std::path::PathBuf> {
    Ok(accounts::accounts_root()?.join("codex"))
}

const LIMITS_PREFIX: &str = ".limits-";

/// Far longer than a read ([`TIMEOUT`] plus a refresh) can take.
const STALE_LIMITS_HOME: Duration = Duration::from_secs(10 * 60);

/// A temporary `CODEX_HOME` for one limits read, made and removed through a
/// handle on its parent so nothing planted redirects either.
struct LimitsHome {
    parent: crate::agent::credential_file::PrivateDir,
    name: String,
    path: std::path::PathBuf,
}

impl Drop for LimitsHome {
    fn drop(&mut self) {
        if let Err(e) = self.parent.remove(&self.name) {
            tracing::warn!(error = %e, "could not remove a limits read's temporary codex home");
        }
    }
}

/// The temporary home for one limits read of the login in `source_home`: the
/// shared config linked in as an agent's overlay has it, and the launch copy
/// of the login. `None` when the host finds the login refused, which reads as
/// signed out like the app-server's own 401, and makes nothing. Removed on
/// every path once made, a failure to fill it included.
fn limits_home(
    source_home: &Path,
    home: &Path,
    refresh: crate::agent::host_login::codex::Refresher<'_>,
    now: i64,
) -> Result<Option<LimitsHome>> {
    use crate::agent::host_login::codex::{self, CodexLoginError};
    let launch = match codex::launch_file(source_home, refresh, now) {
        Ok(launch) => launch,
        Err(CodexLoginError::Revoked) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let root = limits_parent()?;
    std::fs::create_dir_all(&root)?;
    let parent = crate::agent::credential_file::PrivateDir::open(&root)?;
    // A crash mid-read leaves a home behind, a launch copy of a login in it.
    // One older than any read could last is gone by now; a younger one may
    // be another account's read in progress.
    if let Err(e) = parent.remove_stale(LIMITS_PREFIX, STALE_LIMITS_HOME) {
        tracing::warn!(error = %e, "could not sweep stale limits homes");
    }
    let name = format!("{LIMITS_PREFIX}{}", uuid::Uuid::new_v4().simple());
    parent.subdir(&name)?;
    let made = LimitsHome {
        path: root.join(&name),
        parent,
        name,
    };
    crate::agent::codex_home::prepare_overlay(&made.path, home)?;
    crate::agent::codex_home::write_launch(&made.path, launch.as_ref())?;
    Ok(Some(made))
}

fn now() -> i64 {
    super::now_secs()
}

/// The exchange itself, over any line transport — the child's pipes in
/// production, an in-memory pair in tests.
pub async fn converse<R, W>(reader: R, mut writer: W, now: i64) -> Result<RefreshOutcome>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = reader.lines();
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": INITIALIZE_ID,
            "method": "initialize",
            "params": { "clientInfo": {
                "name": "fletch",
                "title": "Fletch",
                "version": env!("CARGO_PKG_VERSION"),
            } },
        }),
    )
    .await?;
    let init = response(&mut lines, INITIALIZE_ID).await?;
    if let Some(err) = init.get("error") {
        return Err(Error::Other(format!(
            "codex app-server refused to start: {}",
            error_message(err)
        )));
    }
    send(
        &mut writer,
        &json!({ "jsonrpc": "2.0", "method": "initialized" }),
    )
    .await?;
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": READ_ID,
            "method": "account/rateLimits/read",
            "params": null,
        }),
    )
    .await?;
    let answer = response(&mut lines, READ_ID).await?;
    outcome(&answer, now)
}

async fn send<W: AsyncWrite + Unpin>(writer: &mut W, message: &Value) -> Result<()> {
    let mut line = message.to_string();
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// The response to request `id`, skipping notifications, the server's own
/// requests (they carry a `method`) and anything that isn't JSON.
async fn response<R: AsyncBufRead + Unpin>(
    lines: &mut tokio::io::Lines<R>,
    id: i64,
) -> Result<Value> {
    while let Some(line) = lines.next_line().await? {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("method").is_none() && message.get("id").and_then(Value::as_i64) == Some(id)
        {
            return Ok(message);
        }
    }
    Err(Error::Other(
        "codex app-server exited before answering".into(),
    ))
}

fn error_message(err: &Value) -> &str {
    err.get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error")
}

/// What the read's response means: a reading, signed out (`-32603` about a
/// 401), or an error worth telling the user about — a network failure must
/// not read as a lost login.
pub fn outcome(response: &Value, now: i64) -> Result<RefreshOutcome> {
    if let Some(err) = response.get("error") {
        let message = error_message(err);
        if err.get("code").and_then(Value::as_i64) == Some(INTERNAL_ERROR_CODE)
            && names_auth_failure(message)
        {
            return Ok(RefreshOutcome::SignedOut);
        }
        return Err(Error::Other(format!(
            "codex could not read its limits: {message}"
        )));
    }
    response
        .pointer("/result/rateLimits")
        .and_then(|limits| from_app_server(limits, now))
        .map(RefreshOutcome::Limits)
        .ok_or_else(|| Error::Other("codex reported no plan limits for this account".into()))
}

#[cfg(test)]
mod tests {
    use super::super::{LimitSource, LimitWindow};
    use super::*;
    use serde_json::json;

    fn jwt_exp(exp: i64) -> String {
        use base64::Engine as _;
        let enc = |v: serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string().as_bytes())
        };
        format!(
            "{}.{}.sig",
            enc(json!({"alg": "RS256"})),
            enc(json!({"iat": exp - 10 * 86_400, "exp": exp}))
        )
    }

    fn never(
        _: &str,
    ) -> std::result::Result<
        crate::agent::host_login::codex::TokenResponse,
        crate::agent::host_login::RefreshFailure,
    > {
        panic!("no refresh expected")
    }

    /// A home a crashed read left behind is swept by the next read; one young
    /// enough to be another read in progress is left alone.
    #[test]
    fn a_read_sweeps_the_homes_crashed_reads_left_behind() {
        accounts::with_test_root(|root| {
            let home = root.join("home");
            std::fs::create_dir_all(&home).unwrap();
            let codex = root.join("codex");
            let stale = codex.join(".limits-crashed");
            let fresh = codex.join(".limits-in-progress");
            for dir in [&stale, &fresh] {
                std::fs::create_dir_all(dir).unwrap();
                std::fs::write(dir.join("auth.json"), "{}").unwrap();
            }
            let old = std::time::SystemTime::now() - Duration::from_secs(3600);
            std::fs::File::open(&stale)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old))
                .unwrap();
            let source = codex.join("work");
            std::fs::create_dir_all(&source).unwrap();

            let made = limits_home(&source, &home, &never, 1_791_448_171).unwrap();

            assert!(!stale.exists());
            assert!(fresh.exists());
            assert!(made.is_some());
        });
    }

    /// The app-server reads limits from a home inside the agent-denied codex
    /// accounts dir, holding the launch copy of the login and the shared
    /// config, never the account's own directory; the home is gone after the
    /// read.
    #[test]
    fn the_limits_read_runs_in_a_hidden_home_with_no_refresh_token() {
        accounts::with_test_root(|root| {
            let home = root.join("home");
            std::fs::create_dir_all(home.join(".codex")).unwrap();
            std::fs::write(home.join(".codex/config.toml"), "").unwrap();
            let source = root.join("codex/work");
            std::fs::create_dir_all(&source).unwrap();
            let now = 1_791_448_171;
            let login = json!({"auth_mode": "chatgpt", "tokens": {
                "access_token": jwt_exp(now + 5 * 86_400), "id_token": "id",
                "refresh_token": "rt.host", "account_id": "a"}});
            std::fs::write(source.join("auth.json"), login.to_string()).unwrap();

            let made = limits_home(&source, &home, &never, now).unwrap().unwrap();

            assert!(
                made.path.starts_with(root.join("codex")),
                "{}",
                made.path.display()
            );
            assert!(made.name.starts_with(".limits-"));
            let written: serde_json::Value =
                serde_json::from_slice(&std::fs::read(made.path.join("auth.json")).unwrap())
                    .unwrap();
            assert_eq!(written["tokens"]["refresh_token"], "");
            assert!(made
                .path
                .join("config.toml")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink());
            let path = made.path.clone();
            drop(made);
            assert!(!path.exists());
            assert_eq!(
                crate::agent::accounts::list_account_ids("codex"),
                vec!["work"]
            );
        });
    }

    /// A `CODEX_HOME` only the login shell exports is the default account's
    /// home: the limits read takes its login from there, not from `~/.codex`.
    #[test]
    fn the_default_limits_read_uses_a_codex_home_the_login_shell_exports() {
        accounts::with_test_root(|root| {
            let home = root.join("home");
            std::fs::create_dir_all(home.join(".codex")).unwrap();
            let shell_home = root.join("shell-codex");
            std::fs::create_dir_all(&shell_home).unwrap();
            let now = 1_791_448_171;
            let login = |account: &str| {
                json!({"auth_mode": "chatgpt", "tokens": {
                    "access_token": jwt_exp(now + 5 * 86_400), "id_token": "id",
                    "refresh_token": "rt.host", "account_id": account}})
                .to_string()
            };
            std::fs::write(home.join(".codex/auth.json"), login("tilde")).unwrap();
            std::fs::write(shell_home.join("auth.json"), login("shell")).unwrap();

            let made = crate::bin_resolve::with_login_shell_env(
                &[("CODEX_HOME", shell_home.to_str().unwrap())],
                || {
                    let source = crate::agent::host_login::codex::source_home(None, &home);
                    limits_home(&source, &home, &never, now)
                },
            )
            .unwrap()
            .unwrap();

            let written: serde_json::Value =
                serde_json::from_slice(&std::fs::read(made.path.join("auth.json")).unwrap())
                    .unwrap();
            assert_eq!(written["tokens"]["account_id"], "shell");
        });
    }

    #[test]
    fn a_refused_login_reads_as_signed_out_without_starting_the_app_server() {
        accounts::with_test_root(|root| {
            let home = root.join("home");
            std::fs::create_dir_all(&home).unwrap();
            let source = root.join("codex/work");
            std::fs::create_dir_all(&source).unwrap();
            let now = 1_791_448_171;
            let login = json!({"auth_mode": "chatgpt", "tokens": {
                "access_token": jwt_exp(now - 1), "id_token": "id",
                "refresh_token": "rt.dead", "account_id": "a"}});
            std::fs::write(source.join("auth.json"), login.to_string()).unwrap();
            let refused = |_: &str| Err(crate::agent::host_login::RefreshFailure::Rejected);

            assert!(limits_home(&source, &home, &refused, now)
                .unwrap()
                .is_none());
        });
    }

    /// Live: one limits read through the real app-server, from a temporary
    /// home built from the login in `$FLETCH_LIVE_CODEX_SOURCE` (a directory
    /// holding a copy of an `auth.json` with more than a day left). Spends no
    /// model quota; the copy is left as it was. Run with:
    ///   FLETCH_LIVE_CODEX_SOURCE=<dir> cargo test --lib live_limits_read -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn live_limits_read_runs_from_a_temporary_home() {
        let Some(source) =
            std::env::var_os("FLETCH_LIVE_CODEX_SOURCE").map(std::path::PathBuf::from)
        else {
            eprintln!("FLETCH_LIVE_CODEX_SOURCE unset; skipping");
            return;
        };
        let before = std::fs::read(source.join("auth.json")).unwrap();
        let RefreshOutcome::Limits(limits) = read_limits(Some(&source)).await.unwrap() else {
            panic!("expected limits");
        };
        println!(
            "five_hour={:?} seven_day={:?}",
            limits.five_hour.map(|w| w.percent),
            limits.seven_day.map(|w| w.percent)
        );
        assert_eq!(std::fs::read(source.join("auth.json")).unwrap(), before);
    }

    const READ_FIXTURE: &str = r#"{"jsonrpc":"2.0","id":2,"result":{"rateLimits":{"primary":{"usedPercent":42,"windowDurationMins":300,"resetsAt":1788265323},"secondary":{"usedPercent":61,"windowDurationMins":10080,"resetsAt":1788765541},"planType":"plus"}}}"#;

    #[test]
    fn a_read_result_becomes_an_app_server_reading() {
        let response: Value = serde_json::from_str(READ_FIXTURE).unwrap();
        let RefreshOutcome::Limits(limits) = outcome(&response, 100).unwrap() else {
            panic!("expected a reading");
        };
        assert_eq!(limits.source, LimitSource::AppServer);
        assert_eq!(limits.as_of, 100);
        assert_eq!(
            limits.five_hour,
            Some(LimitWindow {
                percent: 42.0,
                resets_at: Some(1_788_265_323)
            })
        );
        assert_eq!(
            limits.seven_day,
            Some(LimitWindow {
                percent: 61.0,
                resets_at: Some(1_788_765_541)
            })
        );
    }

    #[test]
    fn the_internal_error_code_reads_as_signed_out() {
        let response = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "error": { "code": -32603, "message": "failed to fetch codex rate limits: 401" },
        });
        assert_eq!(outcome(&response, 0).unwrap(), RefreshOutcome::SignedOut);
    }

    #[test]
    fn an_internal_error_about_something_else_is_not_signed_out() {
        let response = json!({
            "id": 2,
            "error": { "code": -32603, "message": "failed to fetch codex rate limits: connection refused" },
        });
        let err = outcome(&response, 0).unwrap_err().to_string();
        assert!(err.contains("connection refused"), "{err}");
    }

    #[test]
    fn any_other_error_is_reported_with_the_servers_message() {
        let response = json!({
            "id": 2,
            "error": { "code": -32601, "message": "method not found" },
        });
        let err = outcome(&response, 0).unwrap_err().to_string();
        assert!(err.contains("method not found"), "{err}");
    }

    #[test]
    fn a_result_without_limits_is_an_error() {
        let response = json!({ "id": 2, "result": { "rateLimits": null } });
        assert!(outcome(&response, 0).is_err());
    }

    /// The whole exchange against a scripted server: the client's requests go
    /// out in order, and notifications between the answers are skipped.
    #[tokio::test]
    async fn the_exchange_initializes_then_reads() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client);
        let (server_read, mut server_write) = tokio::io::split(server);

        let script = tokio::spawn(async move {
            let mut requests = BufReader::new(server_read).lines();
            let mut seen = Vec::new();
            seen.push(requests.next_line().await.unwrap().unwrap());
            for line in [r#"{"jsonrpc":"2.0","id":1,"result":{"userAgent":"codex"}}"#] {
                server_write
                    .write_all(format!("{line}\n").as_bytes())
                    .await
                    .unwrap();
            }
            seen.push(requests.next_line().await.unwrap().unwrap());
            seen.push(requests.next_line().await.unwrap().unwrap());
            for line in [
                r#"{"jsonrpc":"2.0","method":"account/updated","params":{}}"#,
                "not json",
                READ_FIXTURE,
            ] {
                server_write
                    .write_all(format!("{line}\n").as_bytes())
                    .await
                    .unwrap();
            }
            seen
        });

        let outcome = converse(BufReader::new(client_read), client_write, 5)
            .await
            .unwrap();
        assert!(matches!(outcome, RefreshOutcome::Limits(_)));

        let seen: Vec<Value> = script
            .await
            .unwrap()
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(seen[0]["method"], "initialize");
        assert_eq!(seen[0]["params"]["clientInfo"]["name"], "fletch");
        assert_eq!(seen[1]["method"], "initialized");
        assert!(seen[1].get("id").is_none(), "a notification has no id");
        assert_eq!(seen[2]["method"], "account/rateLimits/read");
        assert_eq!(seen[2]["id"], 2);
    }

    #[tokio::test]
    async fn a_server_that_is_gone_is_an_error() {
        let (client, server) = tokio::io::duplex(1024);
        drop(server);
        let (read, write) = tokio::io::split(client);
        assert!(converse(BufReader::new(read), write, 0).await.is_err());
    }
}
