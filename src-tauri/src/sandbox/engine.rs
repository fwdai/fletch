use std::path::Path;

use crate::error::Result;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    SandboxExec,
    Docker,
}

#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AgentLaunchCtx<'a> {
    pub agent_id: &'a str,
    pub writable_root: &'a Path,
    pub rpc_dir: &'a Path,
    pub cwd: &'a Path,
    pub home: &'a Path,
    pub interactive: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum Keepalive {
    None,
    Profile {
        /// Held for its Drop side-effect (deletes the sandbox profile tempfile).
        /// Skipped by serde: the handle is process-local and can't cross boundaries.
        #[serde(skip)]
        file: Option<tempfile::NamedTempFile>,
    },
}

#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum KillPlan {
    ProcessGroup,
    Container { name: String },
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct LaunchPlan {
    pub program: std::path::PathBuf,
    pub prefix_args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub keepalive: Keepalive,
    pub kill: KillPlan,
}

pub trait SandboxEngine: Send + Sync {
    fn kind(&self) -> EngineKind;
    fn launch_agent(&self, ctx: &AgentLaunchCtx, agent_bin: &str) -> Result<LaunchPlan>;
    fn kill(&self, plan: &KillPlan) -> Result<()>;
    fn is_alive(&self, plan: &KillPlan) -> bool;
}
