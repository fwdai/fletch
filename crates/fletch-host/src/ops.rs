//! What the admin socket can ask for.
//!
//! These are the operator's operations, not a paired device's: they manage the
//! host itself (who is paired, what is waiting for an answer, which projects
//! exist) and they are reachable only through a 0600 socket. So they do not go
//! through `remote::dispatch`'s allowlist — that list is the surface a *device*
//! gets, and widening it would hand a paired phone the ability to pair other
//! phones. Each arm calls the same engine function the desktop's equivalent
//! command calls.

use std::path::PathBuf;
use std::sync::Arc;

use fletch_core::host::Engine;
use fletch_core::remote::RemoteState;
use fletch_core::{commands, rpc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::login::Login;

/// The engine plus the little state the socket needs on top of it.
pub struct Admin {
    pub engine: Engine,
    data_dir: PathBuf,
    login: Arc<Login>,
}

/// Answered for an op nobody defined, in the wire protocol's words.
pub const UNKNOWN_OP: &str = "unknown op";

/// What every op that talks to devices needs, and what it says when the
/// listener never came up.
const NO_REMOTE: &str = "remote access is not running on this host";

impl Admin {
    pub fn new(engine: Engine, data_dir: PathBuf) -> Self {
        Self {
            engine,
            data_dir,
            login: Arc::new(Login::new()),
        }
    }

    pub async fn call(&self, op: &str, args: Value) -> Result<Value, String> {
        match op {
            "status" => self.status(),
            "begin_pairing" => {
                let invite = self.remote()?.begin_pairing();
                value(&invite)
            }
            "devices_list" => value(&self.remote()?.status().devices),
            "revoke_device" => {
                let args: DeviceArgs = parse(args)?;
                match self.remote()?.revoke_device(&args.device_id) {
                    // `false` is "no such device", which a CLI should say out
                    // loud rather than report as a successful revoke.
                    Ok(true) => Ok(Value::Null),
                    Ok(false) => Err(format!("no device with id {}", args.device_id)),
                    Err(e) => Err(e.to_string()),
                }
            }
            "approvals_list" => value(&rpc::approval::pending()),
            "answer_publish_approval" => {
                let args: AnswerArgs = parse(args)?;
                rpc::approval::answer(&args.id, args.approved);
                Ok(Value::Null)
            }
            "github_login_start" => self.login.start(self.engine.db.clone()).await,
            "github_login_status" => Ok(self.login.status()),
            "project_add" => {
                let args: PathArgs = parse(args)?;
                let added = commands::add_workspace_repo_impl(&self.engine.supervisor, args.path)
                    .await
                    .map_err(|e| e.to_string());
                // Every other client — a paired phone, a second CLI — learns
                // that the project list changed the same way it does when the
                // desktop adds one.
                let added = commands::announce_workspace(self.engine.ctx.sink.as_ref(), added)?;
                value(&added)
            }
            "project_clone" => {
                let args: CloneArgs = parse(args)?;
                let cloned = commands::clone_repo_impl(
                    &self.engine.supervisor,
                    &args.spec,
                    &args.dest_parent,
                )
                .await
                .map_err(|e| e.to_string());
                let cloned = commands::announce_workspace(self.engine.ctx.sink.as_ref(), cloned)?;
                value(&cloned)
            }
            _ => Err(UNKNOWN_OP.to_string()),
        }
    }

    /// `remote_status`, plus the three things only the process itself knows.
    /// Flattened into one object so `fletch-host status` prints one document.
    fn status(&self) -> Result<Value, String> {
        let mut status = value(&self.remote()?.status())?;
        let Some(object) = status.as_object_mut() else {
            return Err("the host's status did not serialize as an object".to_string());
        };
        object.insert("dataDir".into(), json!(self.data_dir.to_string_lossy()));
        object.insert("pid".into(), json!(std::process::id()));
        object.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
        Ok(status)
    }

    fn remote(&self) -> Result<&Arc<RemoteState>, String> {
        self.engine.remote.as_ref().ok_or(NO_REMOTE.to_string())
    }
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| e.to_string())
}

fn value<T: serde::Serialize>(value: &T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceArgs {
    device_id: String,
}

#[derive(Deserialize)]
struct AnswerArgs {
    id: String,
    approved: bool,
}

#[derive(Deserialize)]
struct PathArgs {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloneArgs {
    spec: String,
    dest_parent: String,
}
