//! Per-launch container policy: the env a containerized agent gets, the mount
//! sources that must exist before `-v` sees them, and the per-provider config /
//! data / auth preparation.
//!
//! Everything a container launch decides *before* it knows which runtime will
//! carry it out. The runtime engines ([`docker::engine`], [`podman::engine`])
//! call [`prepare`] and then hand what comes back to
//! [`run_args`](super::run_args) — so the two runtimes launch byte-identically
//! and a change to launch policy cannot land on only one of them.
//!
//! [`docker::engine`]: crate::sandbox::docker
//! [`podman::engine`]: crate::sandbox::podman

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::sandbox::engine::AgentLaunchCtx;
use crate::sandbox::policy::{codex_home_dir, opencode_config_dir, opencode_data_dir};

use super::config_dir::{
    borrowed_object_stores, codex_home_is_nondefault, nondefault_claude_config_dir,
    xdg_base_is_nondefault,
};
use super::launch_auth::{
    apply_container_auth, prepare_codex_launch, prepare_cursor_launch, prepare_opencode_launch,
    prepare_pi_launch, present_api_keys,
};
use super::run_args::{prepare_config_mount_dir, ProviderMounts, CREDENTIALS_FILE};
use super::ContainerProvider;

/// The launch inputs [`prepare`] resolved: the CLI process env, the object
/// stores to bind read-only, and the owned per-provider mount inputs a
/// [`ProviderMounts`] borrows from.
pub(crate) struct ContainerLaunch {
    /// Env set on the runtime CLI process; forwarded into the container by the
    /// bare `-e NAME` flags `run_args` emits (values never touch argv —
    /// invariant 3). Config-dir vars come first, the resolved auth vars last.
    pub env: Vec<(String, String)>,
    /// Object stores every checkout under the agent's writable root borrows via
    /// git alternates, each bind-mounted read-only at its identical host path.
    pub borrowed_object_stores: Vec<PathBuf>,

    provider: ContainerProvider,
    /// Index into [`env`](Self::env) where the auth tail starts — everything
    /// from here on is a credential name to forward. Only the resolved set is
    /// forwarded: an ambient credential the chain didn't pick must not reach
    /// the container and override the resolved login.
    auth_start: usize,

    claude_config_dir: Option<PathBuf>,
    claude_credentials_rw: bool,
    config_dir_credentials_rw: bool,
    projects_src: Option<PathBuf>,
    codex_config_dir: Option<PathBuf>,
    forward_codex_home: bool,
    oc_data: Option<PathBuf>,
    oc_config: Option<PathBuf>,
    forward_xdg_data_home: bool,
    forward_xdg_config_home: bool,
    pi_data: Option<PathBuf>,
    cursor_data: Option<PathBuf>,
}

impl ContainerLaunch {
    /// The auth var *names* to forward — exactly the tail [`prepare`] appended.
    pub(crate) fn auth_vars(&self) -> Vec<&str> {
        self.env[self.auth_start..]
            .iter()
            .map(|(k, _)| k.as_str())
            .collect()
    }

    /// The launching provider's mount directives, borrowed from the owned
    /// inputs [`prepare`] filled in. Exactly one provider arm ran, so exactly
    /// one variant's inputs are populated.
    pub(crate) fn mounts(&self) -> ProviderMounts<'_> {
        match self.provider {
            ContainerProvider::Claude => ProviderMounts::Claude {
                config_dir: self.claude_config_dir.as_deref(),
                credentials_rw: self.claude_credentials_rw,
                config_dir_credentials_rw: self.config_dir_credentials_rw,
                projects_src: self
                    .projects_src
                    .as_deref()
                    .expect("claude launch must supply a projects_src"),
            },
            ContainerProvider::Codex => ProviderMounts::Codex {
                config_dir: self
                    .codex_config_dir
                    .as_deref()
                    .expect("codex launch must supply a config_dir"),
                forward_home: self.forward_codex_home,
            },
            ContainerProvider::Opencode => ProviderMounts::Opencode {
                data_dir: self
                    .oc_data
                    .as_deref()
                    .expect("opencode launch must supply a data_dir"),
                config_dir: self.oc_config.as_deref(),
                forward_xdg_data_home: self.forward_xdg_data_home,
                forward_xdg_config_home: self.forward_xdg_config_home,
            },
            ContainerProvider::Pi => ProviderMounts::Pi {
                data_dir: self
                    .pi_data
                    .as_deref()
                    .expect("pi launch must supply a data_dir"),
            },
            ContainerProvider::Cursor => ProviderMounts::Cursor {
                data_dir: self
                    .cursor_data
                    .as_deref()
                    .expect("cursor launch must supply a data_dir"),
            },
        }
    }
}

/// Resolve everything a container launch of `provider` needs, creating the
/// mount sources that must exist first. Fails the launch rather than handing a
/// missing or unusable source to `-v`: a bare `-v` on a missing source has the
/// runtime materialize it *root-owned*, which silently breaks the agent's
/// access to its own auth/config/transcripts.
pub(crate) fn prepare(
    ctx: &AgentLaunchCtx,
    provider: ContainerProvider,
) -> Result<ContainerLaunch> {
    // Object stores every checkout under the agent's writable root borrows
    // via git alternates (a --shared clone). Derived from `ctx.source_repos`
    // — Fletch's authoritative record of each checkout's user-owned source
    // repo — NOT from the checkout's own `.git/objects/info/alternates`.
    // That alternates file lives inside the agent's read-write checkout, so
    // a container agent can overwrite it to name any host path and, on a
    // reused-checkout relaunch (resume / switch_view), have that path
    // bind-mounted read-only into its container — defeating ConfinedReads.
    // Deriving from the source repos (which the agent cannot write) mounts
    // exactly what a `--shared` clone borrows without trusting agent state.
    // Spans every tracked repo, not just the primary: a multi-repo agent
    // has one shared clone per repo. Mounted read-only; empty when a source
    // is missing or has no object store. Container engines force Clone-mode
    // workspaces for every provider, so this is provider-agnostic.
    let borrowed_object_stores = borrowed_object_stores(ctx.source_repos);

    let mut env: Vec<(String, String)> = vec![
        ("HOME".into(), ctx.home.to_string_lossy().into_owned()),
        (
            "FLETCH_RPC_DIR".into(),
            ctx.rpc_dir.to_string_lossy().into_owned(),
        ),
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
    ];
    // A workflow step agent's blackboard is bind-mounted at its identical
    // host path (invariant 1, like the RPC mailbox), so `WF_BLACKBOARD` is
    // the same host path under either engine. Pushed before the provider
    // match sets `auth_start` so it forwards as a plain env var, not an
    // auth var.
    if let Some(board) = ctx.blackboard {
        // Fail closed if the blackboard was not provisioned before launch:
        // a bare `-v` on a missing source has the runtime create it *root-owned*,
        // leaving the host-side reader unable to read the agent's verdict/
        // handoff files. Seatbelt already fails here (its `canonicalize`
        // errors on a missing path); match that so an ordering bug in the
        // scheduler surfaces instead of silently corrupting the mount.
        if !board.is_dir() {
            return Err(Error::Other(format!(
                "workflow blackboard not provisioned before launch: {}",
                board.display()
            )));
        }
        env.push((
            crate::workflow::blackboard::WF_BLACKBOARD_ENV.into(),
            board.to_string_lossy().into_owned(),
        ));
    }

    // Owned per-provider mount inputs, borrowed into a `ProviderMounts` by
    // `ContainerLaunch::mounts`. Defaults describe "no config surface"; the
    // matched arm fills in what its provider needs.
    let mut claude_config_dir: Option<PathBuf> = None;
    let mut claude_credentials_rw = false;
    let mut config_dir_credentials_rw = false;
    let mut projects_src: Option<PathBuf> = None;
    let mut codex_config_dir: Option<PathBuf> = None;
    let mut forward_codex_home = false;
    let mut oc_data: Option<PathBuf> = None;
    let mut oc_config: Option<PathBuf> = None;
    let mut forward_xdg_data_home = false;
    let mut forward_xdg_config_home = false;
    let mut pi_data: Option<PathBuf> = None;
    let mut cursor_data: Option<PathBuf> = None;

    // The config-dir env (CLAUDE_CONFIG_DIR / CODEX_HOME / XDG_*) is pushed
    // before this mark so only the *auth* tail is forwarded as auth vars.
    let auth_start;
    match provider {
        ContainerProvider::Claude => {
            let cfg = nondefault_claude_config_dir(ctx.home);

            // Make sure the mount sources exist before we hand them to `-v`.
            // If a source can't be created (a file already sits at the path,
            // a read-only or missing parent, permissions), mounting it anyway
            // would let the runtime recreate it root-owned or fail the bind
            // opaquely — either way claude loses access to its auth/config.
            // Fail the launch with the path instead of a bad mount.
            let claude_dir = ctx.home.join(".claude");
            prepare_config_mount_dir(&claude_dir)?;
            if let Some(dir) = &cfg {
                prepare_config_mount_dir(dir)?;
            }

            // Per-agent host dir backing claude's `projects/` (session
            // transcripts). Bind-mounted read-write over the read-only config
            // dir's `projects/` (see `run_args::push_claude_config_mount`) so
            // `--resume` survives container recreation without exposing the
            // shared `~/.claude/projects` — other agents' transcripts and
            // global memory stay unreachable (invariant 5). Lives under the
            // agent's writable root, so archive teardown's `rm -rf` reclaims
            // it with no separate cleanup. Created before the container runs so
            // the runtime binds an existing source instead of materializing it
            // root-owned.
            let ps = ctx
                .writable_root
                .join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME);
            std::fs::create_dir_all(&ps).map_err(|e| {
                Error::Other(format!(
                    "preparing Docker sandbox projects mount {} failed: {e}",
                    ps.display()
                ))
            })?;

            // The read-only config mounts get a writable `.credentials.json`
            // overlay only when the file already exists: a bare `-v` on a
            // missing source makes the runtime create a root-owned *directory*
            // there, which would break claude's later write of the real file.
            claude_credentials_rw = claude_dir.join(CREDENTIALS_FILE).is_file();
            config_dir_credentials_rw = cfg
                .as_deref()
                .is_some_and(|dir| dir.join(CREDENTIALS_FILE).is_file());
            if let Some(dir) = &cfg {
                env.push((
                    "CLAUDE_CONFIG_DIR".into(),
                    dir.to_string_lossy().into_owned(),
                ));
            }
            claude_config_dir = cfg;
            projects_src = Some(ps);

            auth_start = env.len();
            apply_container_auth(&mut env, super::auth::resolve())?;
        }
        ContainerProvider::Codex => {
            // Codex's config dir is bind-mounted read-write: auth.json token
            // refresh and the session rollout files it writes both need to
            // persist, and writing rollouts at the same host path keeps the
            // host-side transcript reader (`find_codex_rollouts`) working.
            let dir = codex_home_dir(ctx.home);
            // Forward CODEX_HOME only when it points somewhere other than the
            // default `~/.codex` the container already resolves via HOME —
            // mirrors `nondefault_claude_config_dir`.
            forward_codex_home = codex_home_is_nondefault(ctx.home);
            if forward_codex_home {
                env.push(("CODEX_HOME".into(), dir.to_string_lossy().into_owned()));
            }

            auth_start = env.len();
            prepare_codex_launch(
                &mut env,
                &dir,
                std::env::var("OPENAI_API_KEY").ok().as_deref(),
            )?;
            codex_config_dir = Some(dir);
        }
        ContainerProvider::Opencode => {
            // OpenCode's data dir carries the accounts DB / auth.json and the
            // session storage the host transcript reader tails, so it's bound
            // read-write at its identical host path (mirrors codex's ~/.codex).
            let data = opencode_data_dir(ctx.home);
            // Forward XDG_DATA_HOME only when it points somewhere other than
            // the default `~/.local/share` the container resolves via HOME —
            // mirrors codex's CODEX_HOME handling.
            forward_xdg_data_home =
                xdg_base_is_nondefault("XDG_DATA_HOME", ctx.home, ".local/share");
            if forward_xdg_data_home {
                if let Some(v) = std::env::var_os("XDG_DATA_HOME") {
                    env.push(("XDG_DATA_HOME".into(), v.to_string_lossy().into_owned()));
                }
            }
            // The config dir (custom providers + plugin installs opencode
            // writes) is optional: opencode runs without it, and binding a
            // missing source would have the runtime create it root-owned. Mount
            // + forward it only when it already exists.
            let config = opencode_config_dir(ctx.home);
            if config.is_dir() {
                forward_xdg_config_home =
                    xdg_base_is_nondefault("XDG_CONFIG_HOME", ctx.home, ".config");
                if forward_xdg_config_home {
                    if let Some(v) = std::env::var_os("XDG_CONFIG_HOME") {
                        env.push(("XDG_CONFIG_HOME".into(), v.to_string_lossy().into_owned()));
                    }
                }
                oc_config = Some(config);
            }

            auth_start = env.len();
            let api_keys = present_api_keys(|n| std::env::var(n).ok());
            prepare_opencode_launch(&mut env, &data, api_keys)?;
            oc_data = Some(data);
        }
        ContainerProvider::Pi => {
            // Pi keeps everything under `~/.pi` (agent/auth.json,
            // agent/settings.json, agent/sessions/), bound read-write at its
            // identical host path so auth persists and the host transcript
            // reader tails the sessions.
            let data = ctx.home.join(".pi");
            auth_start = env.len();
            let api_keys = present_api_keys(|n| std::env::var(n).ok());
            prepare_pi_launch(&mut env, &data, api_keys)?;
            pi_data = Some(data);
        }
        ContainerProvider::Cursor => {
            // `~/.cursor` is bound read-write at its identical host path so
            // cursor's session transcripts land where the host reader
            // (`agent::cursor_locate`) tails them. It carries no credential
            // (the login token is keychain-bound); auth is CURSOR_API_KEY only.
            let data = ctx.home.join(".cursor");
            auth_start = env.len();
            prepare_cursor_launch(
                &mut env,
                &data,
                std::env::var("CURSOR_API_KEY").ok().as_deref(),
            )?;
            cursor_data = Some(data);
        }
    }

    Ok(ContainerLaunch {
        env,
        borrowed_object_stores,
        provider,
        auth_start,
        claude_config_dir,
        claude_credentials_rw,
        config_dir_credentials_rw,
        projects_src,
        codex_config_dir,
        forward_codex_home,
        oc_data,
        oc_config,
        forward_xdg_data_home,
        forward_xdg_config_home,
        pi_data,
        cursor_data,
    })
}
