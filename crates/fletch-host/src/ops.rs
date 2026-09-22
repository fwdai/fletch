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
use std::sync::{Arc, OnceLock};

use fletch_core::host::Engine;
use fletch_core::remote::RemoteState;
use fletch_core::{commands, rpc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::login::Login;
use crate::provider;

/// The engine plus the little state the socket needs on top of it.
///
/// The engine arrives late: `serve` binds the socket and starts answering on it
/// *before* `host::boot`, so that a client connecting during a slow start (a
/// migration on a big data dir) is answered rather than left waiting. Until
/// [`Admin::attach`] hands the booted engine over, every op says [`STARTING`].
pub struct Admin {
    engine: OnceLock<Engine>,
    data_dir: PathBuf,
    login: Arc<Login>,
}

/// Answered for an op nobody defined, in the wire protocol's words.
pub const UNKNOWN_OP: &str = "unknown op";

/// Answered for every op while the engine is still booting.
pub const STARTING: &str = "the host is still starting";

/// What every op that talks to devices needs, and what it says when the
/// listener never came up.
const NO_REMOTE: &str = "remote access is not running on this host";

impl Admin {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            engine: OnceLock::new(),
            data_dir,
            login: Arc::new(Login::new()),
        }
    }

    /// The engine booted; answer for real from here on. Called once, by
    /// `serve::start`.
    pub fn attach(&self, engine: Engine) {
        // A second engine on one socket is not a thing that happens; if it
        // somehow did, the first one is the one the ops have been answering
        // from and the one to keep.
        let _ = self.engine.set(engine);
    }

    pub async fn call(&self, op: &str, args: Value) -> Result<Value, String> {
        // One rule for the whole surface rather than one per op: these are the
        // operator's questions about a *running* host, and a host that is still
        // booting has no honest answer to any of them.
        let engine = self.engine()?;
        match op {
            "status" => {
                let mut status = self.status(engine)?;
                // Probed, not read off the host's state, so it is added here
                // rather than inside `status` — see the provider block below.
                // `installed_rows`, never `rows`: `status` is what a liveness
                // check calls, and it must not be able to block behind a vendor
                // CLI's `--version`.
                if let Some(object) = status.as_object_mut() {
                    object.insert(
                        "providers".into(),
                        provider::summary(&provider::installed_rows().await),
                    );
                }
                Ok(status)
            }
            // `preset` is the access the code grants (`full` or `control`);
            // absent means `full`, as it did before presets existed.
            "begin_pairing" => {
                let args: PresetArgs = parse(args)?;
                let invite = remote(engine)?
                    .begin_pairing(args.preset.as_deref())
                    .map_err(|e| e.to_string())?;
                value(&invite)
            }
            "devices_list" => value(&remote(engine)?.status().devices),
            "revoke_device" => {
                let args: DeviceArgs = parse(args)?;
                match remote(engine)?.revoke_device(&args.device_id) {
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
            "github_login_start" => self.login.start(engine.db.clone()).await,
            "github_login_status" => Ok(self.login.status()),
            "project_add" => {
                let args: PathArgs = parse(args)?;
                let added = commands::add_workspace_repo_impl(&engine.supervisor, args.path)
                    .await
                    .map_err(|e| e.to_string());
                // Every other client — a paired phone, a second CLI — learns
                // that the project list changed the same way it does when the
                // desktop adds one.
                let added = commands::announce_workspace(engine.ctx.sink.as_ref(), added)?;
                value(&added)
            }
            "project_clone" => {
                let args: CloneArgs = parse(args)?;
                let cloned =
                    commands::clone_repo_impl(&engine.supervisor, &args.spec, &args.dest_parent)
                        .await
                        .map_err(|e| e.to_string());
                let cloned = commands::announce_workspace(engine.ctx.sink.as_ref(), cloned)?;
                value(&cloned)
            }
            // ── The provider CLIs (`crate::provider`) ─────────────────────
            // Operator-only, like everything else here: `provider_login_command`
            // hands back a resolved argv and an environment, which is the
            // machine's own business and never a paired device's.
            "provider_status" => value(&provider::rows().await),
            "provider_login_command" => {
                let args: ProviderArgs = parse(args)?;
                provider::login_spec(&args.id)
            }
            _ => Err(UNKNOWN_OP.to_string()),
        }
    }

    /// `remote_status`, plus what only the process itself knows: where it keeps
    /// its data, who it is, and what the engine behind the socket is actually
    /// doing. Flattened into one object so `fletch-host status` prints one
    /// document.
    ///
    /// The engine facts are the three an operator with no window checks first —
    /// whether any agent is alive, whether the sandbox is the one they meant on
    /// this box, and whether pushes will work — and each is the cheapest read
    /// available: the workspace snapshot the `get_workspace` op already serves,
    /// and two process-global reads.
    fn status(&self, engine: &Engine) -> Result<Value, String> {
        let mut status = value(&remote(engine)?.status())?;
        let Some(object) = status.as_object_mut() else {
            return Err("the host's status did not serialize as an object".to_string());
        };
        object.insert("dataDir".into(), json!(self.data_dir.to_string_lossy()));
        object.insert("pid".into(), json!(std::process::id()));
        object.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
        object.insert("agents".into(), agent_counts(engine));
        object.insert(
            "sandboxEngine".into(),
            json!(fletch_core::sandbox::selected_engine_kind().as_setting()),
        );
        object.insert(
            "githubConnected".into(),
            json!(fletch_core::github::client::token().is_some()),
        );
        object.insert(
            "githubTokenSource".into(),
            json!(fletch_core::github::client::token_source().map(|s| s.as_str())),
        );
        Ok(status)
    }

    /// The booted engine, or what the socket says until it is here.
    fn engine(&self) -> Result<&Engine, String> {
        self.engine.get().ok_or_else(|| STARTING.to_string())
    }
}

/// `{ total, running }` over the live agents — archived ones are history,
/// not load. `current_workspace` overlays the supervisor's in-memory status
/// on the at-rest record, so `running` is the real count of agents mid-turn
/// rather than whatever the database last wrote; `spawning` is counted as
/// running, since from outside it is an agent the host is busy with.
fn agent_counts(engine: &Engine) -> Value {
    let Some(workspace) = engine.supervisor.current_workspace() else {
        return json!({ "total": 0, "running": 0 });
    };
    let live = workspace.agents.iter().filter(|a| a.archive.is_none());
    let total = live.clone().count();
    let running = live
        .filter(|a| {
            matches!(
                a.status,
                fletch_core::workspace::AgentStatus::Running
                    | fletch_core::workspace::AgentStatus::Spawning
            )
        })
        .count();
    json!({ "total": total, "running": running })
}

fn remote(engine: &Engine) -> Result<&Arc<RemoteState>, String> {
    engine.remote.as_ref().ok_or(NO_REMOTE.to_string())
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

/// `begin_pairing`'s one optional argument. Absent is `full`, so a caller that
/// predates presets (an older `fletch-host` against a newer engine) keeps
/// pairing exactly as it did.
#[derive(Deserialize)]
struct PresetArgs {
    #[serde(default)]
    preset: Option<String>,
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

/// A provider id, for the provider ops.
#[derive(Deserialize)]
struct ProviderArgs {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The socket answers from the moment `serve` binds it, which is before the
    /// engine exists. Every op has to have something to say in that window —
    /// this one, promptly — because the alternative is the client waiting out
    /// the whole boot with no idea whether anything is there.
    #[tokio::test]
    async fn an_op_asked_before_the_engine_lands_says_the_host_is_starting() {
        let admin = Admin::new(PathBuf::from("/nonexistent"));
        for op in ["status", "devices_list", "provider_status", "spawn_agent"] {
            assert_eq!(
                admin.call(op, json!({})).await,
                Err(STARTING.to_string()),
                "{op} answered something else while the host was booting"
            );
        }
    }
}
