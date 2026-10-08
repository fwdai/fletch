//! Per-launch container policy: the env a containerized agent gets, the mount
//! sources that must exist before `-v` sees them, and the per-provider config /
//! data / auth preparation — everything decided *before* the runtime is known.
//! Both runtime engines call [`prepare`] and hand the result to
//! [`run_args`](super::run_args), so they launch byte-identically.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::sandbox::engine::AgentLaunchCtx;
use crate::sandbox::policy::{opencode_config_dir, opencode_data_dir};

use super::config_dir::{
    borrowed_object_stores, nondefault_claude_config_dir, xdg_base_is_nondefault,
};
use super::launch_auth::{
    apply_container_auth, prepare_codex_launch, prepare_cursor_launch, prepare_opencode_launch,
    prepare_pi_launch, present_api_keys, NO_ACCOUNT_AUTH_MSG,
};
use super::run_args::{prepare_config_mount_dir, MappedUser, ProviderMounts, CREDENTIALS_FILE};
use super::ContainerProvider;

/// The launch inputs [`prepare`] resolved: the CLI process env, the object
/// stores to bind read-only, and the owned per-provider mount inputs a
/// [`ProviderMounts`] borrows from.
pub(crate) struct ContainerLaunch {
    /// Env set on the runtime CLI process, forwarded into the container by the
    /// bare `-e NAME` flags `run_args` emits (values never touch argv —
    /// invariant 3). Config-dir vars come first, the resolved auth vars last.
    pub env: Vec<(String, String)>,
    /// Object stores every checkout under the agent's writable root borrows via
    /// git alternates, each bound read-only at its identical host path.
    pub borrowed_object_stores: Vec<PathBuf>,

    provider: ContainerProvider,
    /// Index into [`env`](Self::env) where the credential names to forward
    /// begin. Only the resolved set is forwarded: an ambient credential the
    /// chain didn't pick must not reach the container and override the
    /// resolved login.
    auth_start: usize,

    claude_config_dir: Option<PathBuf>,
    claude_credentials_rw: bool,
    config_dir_credentials_rw: bool,
    projects_src: Option<PathBuf>,
    codex_home: Option<PathBuf>,
    codex_shared: Vec<PathBuf>,
    oc_data: Option<PathBuf>,
    oc_config: Option<PathBuf>,
    forward_xdg_data_home: bool,
    forward_xdg_config_home: bool,
    pi_data: Option<PathBuf>,
    cursor_data: Option<PathBuf>,
    /// `(uid:gid, passwd file)` of a uid-mapped launch — see
    /// [`MappedUser`].
    mapped: Option<(String, PathBuf)>,
}

impl ContainerLaunch {
    /// The user the container runs as, with its passwd file, when mapped.
    pub(crate) fn mapped_user(&self) -> Option<MappedUser<'_>> {
        self.mapped
            .as_ref()
            .map(|(user, passwd_file)| MappedUser { user, passwd_file })
    }

    /// The auth var *names* to forward — exactly the tail [`prepare`] appended.
    pub(crate) fn auth_vars(&self) -> Vec<&str> {
        self.env[self.auth_start..]
            .iter()
            .map(|(k, _)| k.as_str())
            .collect()
    }

    /// The launching provider's mount directives. Exactly one provider arm of
    /// [`prepare`] ran, so exactly one variant's inputs are populated.
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
                home: self
                    .codex_home
                    .as_deref()
                    .expect("codex launch must supply its CODEX_HOME"),
                shared: &self.codex_shared,
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
/// mount sources first and failing the launch rather than handing `-v` a
/// missing one — the runtime would materialize it root-owned, silently cutting
/// the agent off from its own auth/config/transcripts. `run_as_user` is the
/// [`RunSpec::run_as_user`](super::run_args::RunSpec::run_as_user) the launch
/// will pass.
pub(crate) fn prepare(
    ctx: &AgentLaunchCtx,
    provider: ContainerProvider,
    run_as_user: Option<&str>,
) -> Result<ContainerLaunch> {
    // Derived from `ctx.source_repos` (every tracked repo, not just the
    // primary), never from the checkout's own `.git/objects/info/alternates`:
    // that file is agent-writable, so a container agent could name any host
    // path there and have it bind-mounted on a reused-checkout relaunch,
    // defeating ConfinedReads.
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
    // Pushed before the provider match sets `auth_start`, so it forwards as a
    // plain env var rather than an auth var.
    if let Some(board) = ctx.blackboard {
        // Fail closed rather than let the runtime materialize a missing source
        // root-owned, which would leave the host-side reader unable to read the
        // agent's verdict/handoff files. Matches seatbelt, whose `canonicalize`
        // already errors here.
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
    // The adopted tree is this launch's `cwd`: bound missing, the runtime would
    // invent an empty root-owned dir and the agent would start in a workspace
    // with none of the run's work in it. Fail with the path instead.
    if let Some(tree) = ctx.adopted_workspace() {
        if !tree.is_dir() {
            return Err(Error::Other(format!(
                "adopted workspace missing at launch: {}",
                tree.display()
            )));
        }
    }

    // The mapped uid is in no passwd file of the image, so anything that looks
    // itself up (`os.userInfo()`, `whoami`) fails; `run_args` binds this over
    // `/etc/passwd`. It lives under this engine's data dir — the one root a
    // container never sees — never under a bind an agent can write.
    let mapped = match run_as_user {
        Some(user) => {
            let data_dir = crate::sandbox::configured_data_dir().ok_or_else(|| {
                Error::Other("uid-mapped launch before the host published its data dir".into())
            })?;
            Some((
                user.to_string(),
                write_passwd_file(&data_dir, user, ctx.home)?,
            ))
        }
        None => None,
    };

    // Owned per-provider mount inputs, borrowed into a `ProviderMounts` by
    // `ContainerLaunch::mounts`. Only the matched arm fills any of these in.
    let mut claude_config_dir: Option<PathBuf> = None;
    let mut claude_credentials_rw = false;
    let mut config_dir_credentials_rw = false;
    let mut projects_src: Option<PathBuf> = None;
    let mut codex_home: Option<PathBuf> = None;
    let mut codex_shared: Vec<PathBuf> = Vec::new();
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
            // Every account runs in the same config dir; a managed one differs
            // only by the token the auth chain below forwards, so its account
            // dir (host-only login storage) is never mounted.
            let cfg = nondefault_claude_config_dir(ctx.home);

            // Fail the launch with the path rather than hand `-v` a source we
            // couldn't create: the bind would either be recreated root-owned or
            // fail opaquely, and claude loses access to its auth/config.
            let claude_dir = ctx.home.join(".claude");
            prepare_config_mount_dir(&claude_dir)?;
            if let Some(dir) = &cfg {
                prepare_config_mount_dir(dir)?;
            }

            // Per-agent host dir backing claude's `projects/` — see
            // `run_args::push_claude_config_mount`. Under the agent's writable
            // root, so archive teardown's `rm -rf` reclaims it.
            let ps = ctx
                .writable_root
                .join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME);
            std::fs::create_dir_all(&ps).map_err(|e| {
                Error::Other(format!(
                    "preparing container sandbox projects mount {} failed: {e}",
                    ps.display()
                ))
            })?;

            // Overlay `.credentials.json` only when the file already exists: on
            // a missing source the runtime creates a root-owned *directory*
            // there, breaking claude's later write of the real file. Never
            // under a host-resolved token: the host is the login's only
            // writer, and a refresh from in here would rotate its refresh
            // token out from under it.
            let host_owned = ctx.oauth_token.is_some();
            claude_credentials_rw = !host_owned && claude_dir.join(CREDENTIALS_FILE).is_file();
            config_dir_credentials_rw = !host_owned
                && cfg
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
            apply_container_auth(&mut env, super::auth::resolve(ctx.oauth_token))?;
        }
        ContainerProvider::Codex => {
            // The host assembled the overlay and wrote its launch credential
            // before this launch (`agent::spawn`); the account's own directory
            // stays on the host.
            let dir = ctx
                .codex_home
                .ok_or_else(|| Error::Other("codex launch without its CODEX_HOME overlay".into()))?
                .to_path_buf();
            env.push(("CODEX_HOME".into(), dir.to_string_lossy().into_owned()));

            auth_start = env.len();
            // An ambient `OPENAI_API_KEY` signs in the default account only:
            // under a managed one it would silently replace that account's
            // login, so only the account's own credential counts.
            let api_key = match ctx.account_dir {
                Some(_) => None,
                None => std::env::var("OPENAI_API_KEY").ok(),
            };
            if ctx.account_dir.is_some() && !dir.join("auth.json").is_file() {
                return Err(Error::Other(NO_ACCOUNT_AUTH_MSG.to_string()));
            }
            prepare_codex_launch(&mut env, &dir, api_key.as_deref())?;
            // From the host's own resolution of the shared source, never from
            // the overlay's links, which the agent can rewrite.
            let source = crate::agent::accounts::shared_source_dir("codex", ctx.home);
            codex_shared = crate::agent::accounts::shared_items("codex")
                .iter()
                .map(|item| source.join(item))
                .filter(|path| path.exists())
                .collect();
            codex_home = Some(dir);
        }
        ContainerProvider::Opencode => {
            let data = opencode_data_dir(ctx.home);
            // Forwarded only when non-default, as with CODEX_HOME above.
            forward_xdg_data_home =
                xdg_base_is_nondefault("XDG_DATA_HOME", ctx.home, ".local/share");
            if forward_xdg_data_home {
                if let Some(v) = std::env::var_os("XDG_DATA_HOME") {
                    env.push(("XDG_DATA_HOME".into(), v.to_string_lossy().into_owned()));
                }
            }
            // Optional — opencode runs without it, and binding a missing source
            // would have the runtime create it root-owned.
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
            // Pi keeps auth, settings and sessions all under `~/.pi`.
            let data = ctx.home.join(".pi");
            auth_start = env.len();
            let api_keys = present_api_keys(|n| std::env::var(n).ok());
            prepare_pi_launch(&mut env, &data, api_keys)?;
            pi_data = Some(data);
        }
        ContainerProvider::Cursor => {
            // No credential lives here (the login token is keychain-bound);
            // auth is CURSOR_API_KEY only.
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
        codex_home,
        codex_shared,
        oc_data,
        oc_config,
        forward_xdg_data_home,
        forward_xdg_config_home,
        pi_data,
        cursor_data,
        mapped,
    })
}

/// The `/etc/passwd` of a uid-mapped container: root, plus the mapped `user`
/// (`uid:gid`) with the container's `$HOME`.
fn passwd_contents(user: &str, home: &Path) -> String {
    format!(
        "root:x:0:0:root:/root:/bin/sh\nfletch:x:{user}:fletch:{}:/bin/sh\n",
        home.display()
    )
}

/// Write [`passwd_contents`] to `<data_dir>/sandbox/passwd` and return that
/// path for `run_args` to bind. The daemon resolves a bind source as root, so
/// the file must sit where no agent can swap it for a symlink between this
/// write and the mount: the data dir is the engine's own (0700, never bound
/// into a container), unlike the agent's writable root or RPC dir.
///
/// Replaced atomically (temp file in the same dir, then rename): every mapped
/// launch rewrites this one path, and a running container has the previous
/// file bind-mounted as its `/etc/passwd` — a truncating write would leave it
/// empty mid-write, the very lookup failure the file exists to prevent. The
/// old inode stays intact for the containers holding it; new launches bind
/// the new one.
pub(crate) fn write_passwd_file(data_dir: &Path, user: &str, home: &Path) -> Result<PathBuf> {
    use std::io::Write;
    let dir = data_dir.join("sandbox");
    let path = dir.join("passwd");
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        tmp.write_all(passwd_contents(user, home).as_bytes())?;
        tmp.persist(&path)?;
        Ok(())
    };
    write().map_err(|e| {
        Error::Other(format!(
            "preparing container passwd file {} failed: {e}",
            path.display()
        ))
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mapped uid must resolve (`getpwuid`) to a user whose home is the
    /// container's `$HOME`; root keeps its entry.
    #[test]
    fn passwd_names_root_and_the_mapped_user() {
        assert_eq!(
            passwd_contents("12345:12345", Path::new("/home/svc")),
            "root:x:0:0:root:/root:/bin/sh\nfletch:x:12345:12345:fletch:/home/svc:/bin/sh\n",
        );
    }

    /// A codex launch under a managed account mounts and forwards the
    /// agent's overlay, gates on the overlay's launch credential, mounts
    /// nothing of the account's own directory, and never lets an ambient
    /// `OPENAI_API_KEY` (the default account's) stand in for the login.
    #[test]
    fn a_codex_account_launch_mounts_the_overlay_not_the_account_home() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path().join("home");
        let root = td.path().join("w");
        let rpc = td.path().join("rpc");
        let account = td.path().join("accounts/codex/work");
        let overlay = root.join(".fletch-codex-home");
        for dir in [&home, &root, &rpc, &account, &overlay] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(account.join("auth.json"), "{}").unwrap();
        let ctx = AgentLaunchCtx {
            agent_id: "a1",
            provider: "codex",
            writable_root: &root,
            source_repos: &[],
            rpc_dir: &rpc,
            cwd: &root,
            home: &home,
            interactive: false,
            blackboard: None,
            account_dir: Some(&account),
            oauth_token: None,
            codex_home: Some(&overlay),
        };

        let err = prepare(&ctx, ContainerProvider::Codex, None)
            .err()
            .expect("an overlay with no launch credential must not launch");
        assert!(err.to_string().contains("account isn't signed in"), "{err}");

        std::fs::write(overlay.join("auth.json"), "{}").unwrap();
        let launch = prepare(&ctx, ContainerProvider::Codex, None).unwrap();
        let overlay_s = overlay.to_string_lossy().into_owned();
        assert!(launch
            .env
            .iter()
            .any(|(k, v)| k == "CODEX_HOME" && *v == overlay_s));
        assert!(launch.auth_vars().is_empty(), "{:?}", launch.auth_vars());
        match launch.mounts() {
            ProviderMounts::Codex { home: mounted, .. } => {
                assert_eq!(mounted, overlay.as_path());
            }
            _ => panic!("expected codex mounts"),
        }
        let account_s = account.to_string_lossy().into_owned();
        assert!(!launch.env.iter().any(|(_, v)| v.contains(&account_s)));
    }

    /// The shared config is mounted from the host's own codex home, read-only,
    /// whatever links the agent left in its overlay.
    #[cfg(unix)]
    #[test]
    fn a_codex_launch_mounts_the_shared_config_the_host_resolves() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path().join("home");
        let root = td.path().join("w");
        let rpc = td.path().join("rpc");
        let overlay = root.join(".fletch-codex-home");
        for dir in [&home.join(".codex/skills"), &root, &rpc, &overlay] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(home.join(".codex/config.toml"), "").unwrap();
        std::fs::write(overlay.join("auth.json"), "{}").unwrap();
        std::os::unix::fs::symlink("/etc", overlay.join("prompts")).unwrap();
        let ctx = AgentLaunchCtx {
            agent_id: "a1",
            provider: "codex",
            writable_root: &root,
            source_repos: &[],
            rpc_dir: &rpc,
            cwd: &root,
            home: &home,
            interactive: false,
            blackboard: None,
            account_dir: None,
            codex_home: Some(&overlay),
            oauth_token: None,
        };

        let launch = prepare(&ctx, ContainerProvider::Codex, None).unwrap();

        match launch.mounts() {
            ProviderMounts::Codex { shared, .. } => {
                let source = crate::sandbox::policy::codex_home_dir(&home);
                assert_eq!(
                    shared,
                    &[source.join("config.toml"), source.join("skills")][..]
                );
            }
            _ => panic!("expected codex mounts"),
        }
    }

    /// A claude launch forwards the host-resolved token as the one auth var,
    /// never points claude at another config dir, and leaves the host's
    /// credentials file read-only (the host is its only writer).
    #[test]
    fn a_claude_launch_forwards_the_host_token_and_no_account_dir() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path().join("home");
        let root = td.path().join("w");
        let rpc = td.path().join("rpc");
        for dir in [&home, &root, &rpc] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude").join(CREDENTIALS_FILE), "{}").unwrap();
        let token = crate::agent::host_login::claude::AccessToken::for_test("sk-ant-oat-test", 1);
        let ctx = AgentLaunchCtx {
            agent_id: "a1",
            provider: "claude",
            writable_root: &root,
            source_repos: &[],
            rpc_dir: &rpc,
            cwd: &root,
            home: &home,
            interactive: false,
            blackboard: None,
            account_dir: None,
            oauth_token: Some(&token),
            codex_home: None,
        };

        let launch = prepare(&ctx, ContainerProvider::Claude, None).unwrap();
        assert_eq!(launch.auth_vars()[0], "CLAUDE_CODE_OAUTH_TOKEN");
        assert!(!launch
            .auth_vars()
            .iter()
            .any(|v| *v == "ANTHROPIC_API_KEY" || *v == "ANTHROPIC_AUTH_TOKEN"));
        assert!(launch
            .env
            .iter()
            .any(|(k, v)| k == "CLAUDE_CODE_OAUTH_TOKEN" && v == "sk-ant-oat-test"));
        match launch.mounts() {
            ProviderMounts::Claude { credentials_rw, .. } => assert!(!credentials_rw),
            _ => panic!("expected claude mounts"),
        }
    }

    /// A `CLAUDE_CONFIG_DIR` only the login shell exports is the dir the host
    /// login comes from, so the container mounts and forwards that dir.
    #[test]
    fn a_claude_launch_mounts_and_forwards_the_login_shells_config_dir() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path().join("home");
        let root = td.path().join("w");
        let rpc = td.path().join("rpc");
        let relocated = td.path().join("claude-eve");
        for dir in [&home, &root, &rpc] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let token = crate::agent::host_login::claude::AccessToken::for_test("sk-ant-oat-test", 1);
        let ctx = AgentLaunchCtx {
            agent_id: "a1",
            provider: "claude",
            writable_root: &root,
            source_repos: &[],
            rpc_dir: &rpc,
            cwd: &root,
            home: &home,
            interactive: false,
            blackboard: None,
            account_dir: None,
            oauth_token: Some(&token),
            codex_home: None,
        };

        let launch = crate::bin_resolve::with_login_shell_env(
            &[("CLAUDE_CONFIG_DIR", relocated.to_str().unwrap())],
            || prepare(&ctx, ContainerProvider::Claude, None),
        )
        .unwrap();

        let relocated_s = relocated.to_string_lossy().into_owned();
        assert!(launch
            .env
            .iter()
            .any(|(k, v)| k == "CLAUDE_CONFIG_DIR" && *v == relocated_s));
        match launch.mounts() {
            ProviderMounts::Claude { config_dir, .. } => {
                assert_eq!(config_dir, Some(relocated.as_path()));
            }
            _ => panic!("expected claude mounts"),
        }
    }

    /// The file lands under the data dir handed in — never under a path an
    /// agent's container has read-write — and is rewritten in place each time.
    #[test]
    fn passwd_file_lives_under_the_data_dir() {
        let td = tempfile::tempdir().unwrap();
        let path = write_passwd_file(td.path(), "1000:1000", Path::new("/home/svc")).unwrap();
        assert_eq!(path, td.path().join("sandbox").join("passwd"));
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("fletch:x:1000:1000:"));
        // A second launch as another uid replaces, not appends — by rename, so
        // a container still holding the first file (an open handle here stands
        // in for its bind mount) keeps the full old contents throughout.
        let mut held = std::fs::File::open(&path).unwrap();
        let again = write_passwd_file(td.path(), "2000:2000", Path::new("/home/svc")).unwrap();
        let written = std::fs::read_to_string(&again).unwrap();
        assert!(written.contains("fletch:x:2000:2000:"), "{written}");
        assert!(!written.contains("1000"), "{written}");
        let mut old = String::new();
        std::io::Read::read_to_string(&mut held, &mut old).unwrap();
        assert!(old.contains("fletch:x:1000:1000:"), "{old}");
        // Nothing but the live file is left behind: no temp files.
        let entries: Vec<_> = std::fs::read_dir(td.path().join("sandbox"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("passwd")]);
    }
}
