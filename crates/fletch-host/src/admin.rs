//! The host's local control socket.
//!
//! A Unix domain socket at `<data_dir>/host.sock`, mode 0600 inside a 0700 data
//! dir. Frames carry no credential and the socket takes no handshake: the file
//! permission *is* the authentication, exactly as it is for the SQLite database
//! sitting beside it. Anyone who can open this socket can already read the
//! host's database and its device store.
//!
//! The envelope is the wire protocol's (`docs/remote-protocol.md`):
//! `{ id, op, args }` in, `{ id, ok, result }` or `{ id, ok: false, error }`
//! back, one JSON document per line. Deliberately the same shape rather than a
//! second protocol — the ops differ (these are the operator's, not a paired
//! device's) but nothing about parsing, ids or error reporting has to be learned
//! twice.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::ops::Admin;

/// The socket's name inside the data dir.
pub const SOCKET_FILE: &str = "host.sock";

/// The data dir is the host's whole trust boundary — database, device store,
/// host key, and this socket — so it is the service user's and nobody else's.
pub const DIR_MODE: u32 = 0o700;

/// Owner read/write. Set immediately after the bind; the window before that is
/// covered by [`DIR_MODE`] on the directory the socket is created in.
pub const SOCKET_MODE: u32 = 0o600;

/// What every client subcommand says when there is nothing on the other end.
pub const NOT_RUNNING: &str = "fletch-host is not running; start it with `fletch-host serve`";

pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SOCKET_FILE)
}

/// One request. `id` is the client's, echoed back on the answer; a CLI that
/// sends one request per connection has no use for it, but a client that
/// pipelines does, and matching the wire protocol costs nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub op: String,
    #[serde(default = "empty_args")]
    pub args: Value,
}

fn empty_args() -> Value {
    Value::Object(Default::default())
}

/// One answer. `result` and `error` are exclusive, and `ok` says which.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Bind the socket, ready to accept.
///
/// A socket file left behind by a host that died is removed first — but only
/// after checking that nothing answers on it, so a second `serve` against the
/// same data dir refuses instead of quietly stealing the first one's socket
/// (two engines on one SQLite database is the failure this prevents).
pub async fn bind(data_dir: &Path) -> Result<UnixListener, String> {
    let path = socket_path(data_dir);
    if path.exists() {
        if UnixStream::connect(&path).await.is_ok() {
            return Err(format!(
                "another fletch-host is already serving {}",
                data_dir.display()
            ));
        }
        std::fs::remove_file(&path)
            .map_err(|e| format!("cannot remove the stale socket {}: {e}", path.display()))?;
    }
    let listener =
        UnixListener::bind(&path).map_err(|e| format!("cannot bind {}: {e}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(SOCKET_MODE))
        .map_err(|e| format!("cannot set the mode of {}: {e}", path.display()))?;
    Ok(listener)
}

/// Answer requests until the process ends. One task per connection, one
/// request at a time within it: nothing here is hot, and a client that wants
/// two answers at once can open two connections.
pub async fn serve(admin: Arc<Admin>, listener: UnixListener) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => {
                // A single failed accept is not worth ending the host over; a
                // listener that is broken for good would spin, so log every one.
                tracing::warn!(error = %e, "admin socket: accept failed");
                continue;
            }
        };
        let admin = admin.clone();
        tokio::spawn(async move {
            if let Err(e) = session(admin, stream).await {
                tracing::debug!(error = %e, "admin socket: connection ended");
            }
        });
    }
}

async fn session(admin: Arc<Admin>, stream: UnixStream) -> io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                let id = request.id.clone();
                tracing::debug!(op = %request.op, "admin socket: request");
                match admin.call(&request.op, request.args).await {
                    Ok(result) => Response {
                        id,
                        ok: true,
                        result: Some(result),
                        error: None,
                    },
                    Err(error) => Response {
                        id,
                        ok: false,
                        result: None,
                        error: Some(error),
                    },
                }
            }
            // No id to answer with, so the reply carries an empty one rather
            // than nothing at all: a client waiting on a line gets its error
            // instead of a timeout.
            Err(e) => Response {
                id: String::new(),
                ok: false,
                result: None,
                error: Some(format!("malformed request: {e}")),
            },
        };
        let mut frame = serde_json::to_vec(&response).unwrap_or_else(|_| {
            br#"{"id":"","ok":false,"error":"the host could not serialize its answer"}"#.to_vec()
        });
        frame.push(b'\n');
        write.write_all(&frame).await?;
        write.flush().await?;
    }
    Ok(())
}

/// Ask the running host one question. The client half of everything above.
///
/// Bounded, because connecting is not the same as being answered: `serve` binds
/// this socket *before* the engine boots, so a client that asks during a long
/// database migration connects to a listener nothing is accepting on yet and
/// would otherwise wait for as long as the migration takes. Every subcommand
/// goes through here, so every one of them gets an answer or an error.
pub async fn call(data_dir: &Path, op: &str, args: Value) -> Result<Value, String> {
    let path = socket_path(data_dir);
    match tokio::time::timeout(CALL_TIMEOUT, exchange(&path, op, args)).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "{} did not answer in {}s; the host is still starting (a database migration on a \
             big data dir can take a while) — try again in a moment",
            path.display(),
            CALL_TIMEOUT.as_secs()
        )),
    }
}

/// How long [`call`] waits for the whole connect-ask-answer. Generous: it is
/// there to end a wait that has no end, not to time an op out.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(15);

async fn exchange(path: &Path, op: &str, args: Value) -> Result<Value, String> {
    let stream = UnixStream::connect(&path)
        .await
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => NOT_RUNNING.to_string(),
            _ => format!("cannot reach {}: {e}", path.display()),
        })?;
    let (read, mut write) = stream.into_split();

    let mut frame = serde_json::to_vec(&Request {
        id: "1".to_string(),
        op: op.to_string(),
        args,
    })
    .map_err(|e| e.to_string())?;
    frame.push(b'\n');
    write.write_all(&frame).await.map_err(|e| e.to_string())?;
    write.flush().await.map_err(|e| e.to_string())?;

    let line = BufReader::new(read)
        .lines()
        .next_line()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the host closed the socket without answering".to_string())?;
    let response: Response = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    if response.ok {
        Ok(response.result.unwrap_or(Value::Null))
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "the host refused the request".to_string()))
    }
}
