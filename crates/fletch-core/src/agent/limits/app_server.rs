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
/// A managed account runs with `CODEX_HOME` pointed at its directory and the
/// default account's `OPENAI_API_KEY` removed, so it answers with its own
/// login or none — never the default's.
pub async fn read_limits(account_dir: Option<&Path>) -> Result<RefreshOutcome> {
    let home =
        dirs::home_dir().ok_or_else(|| Error::Other("HOME directory not available".into()))?;
    let (bin, label) = crate::agent::provider_bin_label("codex")
        .ok_or_else(|| Error::Other("codex has no binary name".into()))?;
    let program = crate::agent::resolve_agent_bin("codex", bin, label, &home)?;

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
    if let Some(dir) = account_dir {
        cmd.env("CODEX_HOME", dir);
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
    answer
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
