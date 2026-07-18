use super::*;

use std::path::{Path, PathBuf};

use super::super::auth::ContainerAuth;
use super::auth::{
    codex_auth_env, cursor_auth_env, multi_provider_auth_env, NO_CODEX_AUTH_MSG,
    NO_CONTAINER_AUTH_MSG, NO_CURSOR_AUTH_MSG, NO_OPENCODE_AUTH_MSG, NO_PI_AUTH_MSG,
};
use super::config_dir::config_dir_is_default;
use super::util::container_running;

/// The version-refresh loop guard: exact-pair matching, per-provider
/// isolation, persistence callback on record, and safe recording before
/// `init` is ever called. Touches the process-wide `VERSION_GUARD`
/// static — the only test that does, so no serialization needed (the
/// same shared-global contract as `progress`'s sink test).
#[test]
fn version_refresh_guard_round_trip() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // Pre-init (headless/tests): nothing attempted, recording is safe and
    // guards the current process even without a persister.
    assert!(!version_refresh_attempted("claude", "v1@fletch-agent:aaa"));
    record_version_refresh("claude", "v1@fletch-agent:aaa".into());
    assert!(version_refresh_attempted("claude", "v1@fletch-agent:aaa"));

    // Init replaces state wholesale (the app seeds from the settings row).
    let persisted = Arc::new(AtomicUsize::new(0));
    let count = persisted.clone();
    init_version_refresh_guard(
        [("codex".to_string(), "v2@fletch-agent-codex:bbb".to_string())].into(),
        move |map| {
            count.store(map.len(), Ordering::SeqCst);
        },
    );
    assert!(version_refresh_attempted(
        "codex",
        "v2@fletch-agent-codex:bbb"
    ));
    // Exact pair only: a new host version or a new tag re-arms the trigger.
    assert!(!version_refresh_attempted(
        "codex",
        "v3@fletch-agent-codex:bbb"
    ));
    assert!(!version_refresh_attempted(
        "codex",
        "v2@fletch-agent-codex:ccc"
    ));
    // Per-provider isolation.
    assert!(!version_refresh_attempted(
        "claude",
        "v2@fletch-agent-codex:bbb"
    ));

    // Recording persists the whole map through the installed callback,
    // and one pair per provider suffices (newer replaces older).
    record_version_refresh("claude", "v9@fletch-agent:ddd".into());
    assert_eq!(
        persisted.load(Ordering::SeqCst),
        2,
        "persister sees both providers"
    );
    record_version_refresh("claude", "v10@fletch-agent:ddd".into());
    assert!(version_refresh_attempted("claude", "v10@fletch-agent:ddd"));
    assert!(!version_refresh_attempted("claude", "v9@fletch-agent:ddd"));
    assert_eq!(
        persisted.load(Ordering::SeqCst),
        2,
        "replaced, not accumulated"
    );
}

/// The per-agent claude transcript dir every claude spec shares.
const CLAUDE_PROJECTS_SRC: &str = "/Users/u/.fletch/worktrees/orkney/.fletch-claude-projects";

/// Claude mount directives with the two carve-out knobs the argv tests flex.
fn claude_mounts<'a>(
    config_dir: Option<&'a Path>,
    credentials_rw: bool,
    config_dir_credentials_rw: bool,
) -> ProviderMounts<'a> {
    ProviderMounts::Claude {
        config_dir,
        credentials_rw,
        config_dir_credentials_rw,
        projects_src: Path::new(CLAUDE_PROJECTS_SRC),
    }
}

fn test_spec<'a>(interactive: bool) -> RunSpec<'a> {
    RunSpec {
        interactive,
        name: "fletch-orkney-deadbeef",
        agent_id: "orkney",
        writable_root: Path::new("/Users/u/.fletch/worktrees/orkney"),
        rpc_dir: Path::new("/Users/u/.fletch/rpc/orkney"),
        home: Path::new("/Users/u"),
        cwd: Path::new("/Users/u/.fletch/worktrees/orkney/repo"),
        blackboard: None,
        mounts: claude_mounts(None, false, false),
        borrowed_object_stores: &[],
        memory: "4g",
        cpus: "2",
        image: "fletch-agent:abc123def456",
        agent_bin: "claude",
        auth_vars: &[
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_AUTH_TOKEN",
        ],
    }
}

/// A `RunSpec` for a read-write-config provider (codex/opencode/pi): no claude
/// config surface, `mounts`/`image`/`agent_bin`/`auth_vars` supplied by the
/// caller. Keeps the codex/opencode/pi argv tests to their own differences.
fn rw_config_spec<'a>(
    mounts: ProviderMounts<'a>,
    image: &'a str,
    agent_bin: &'a str,
    auth_vars: &'a [&'a str],
) -> RunSpec<'a> {
    RunSpec {
        interactive: false,
        name: "fletch-orkney-deadbeef",
        agent_id: "orkney",
        writable_root: Path::new("/Users/u/.fletch/worktrees/orkney"),
        rpc_dir: Path::new("/Users/u/.fletch/rpc/orkney"),
        home: Path::new("/Users/u"),
        cwd: Path::new("/Users/u/.fletch/worktrees/orkney/repo"),
        blackboard: None,
        mounts,
        borrowed_object_stores: &[],
        memory: "4g",
        cpus: "2",
        image,
        agent_bin,
        auth_vars,
    }
}

/// A codex `RunSpec`: a read-write `~/.codex` mount, `OPENAI_API_KEY` as the
/// forwarded auth var, and the codex image.
fn codex_spec<'a>() -> RunSpec<'a> {
    rw_config_spec(
        ProviderMounts::Codex {
            config_dir: Path::new("/Users/u/.codex"),
            forward_home: false,
        },
        "fletch-agent-codex:abc123def456",
        "codex",
        &["OPENAI_API_KEY"],
    )
}

/// Two-token flag lookup: the value following `flag` each time it appears.
fn values_of<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
    args.windows(2)
        .filter(|w| w[0] == flag)
        .map(|w| w[1].as_str())
        .collect()
}

#[test]
fn argv_mounts_exactly_the_three_dirs_at_identical_paths() {
    // Workspace + mailbox read-write; `~/.claude` read-only (invariant 5),
    // followed by the read-write per-agent `projects/` transcript overlay.
    // No credentials file in this spec, so no credentials overlay; no
    // borrowed object stores, so no `.git/objects` RO mount either.
    let args = run_args(&test_spec(false));
    assert_eq!(
        values_of(&args, "-v"),
        vec![
            "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
            "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
            "/Users/u/.claude:/Users/u/.claude:ro",
            "/Users/u/.fletch/worktrees/orkney/.fletch-claude-projects:/Users/u/.claude/projects",
        ],
    );
    assert!(
        !args.iter().any(|a| a.contains("/objects")),
        "no object-store mount without borrowed stores"
    );
    assert_eq!(
        values_of(&args, "-w"),
        vec!["/Users/u/.fletch/worktrees/orkney/repo"],
    );
}

#[test]
fn argv_mounts_blackboard_and_forwards_its_env_when_present() {
    let board = Path::new("/Users/u/.fletch/runs/run-1/blackboard");
    let mut spec = test_spec(false);
    spec.blackboard = Some(board);
    let args = run_args(&spec);

    // Bound read-write at its identical host path (invariant 1), the same
    // shape as the RPC mailbox mount.
    assert!(
        values_of(&args, "-v").contains(
            &"/Users/u/.fletch/runs/run-1/blackboard:/Users/u/.fletch/runs/run-1/blackboard"
        ),
        "blackboard must be bind-mounted at its identical host path, got {:?}",
        values_of(&args, "-v")
    );
    // Forwarded so the in-container agent finds the mount via `$WF_BLACKBOARD`.
    assert!(
        values_of(&args, "-e").contains(&"WF_BLACKBOARD"),
        "WF_BLACKBOARD must be forwarded into the container"
    );
}

#[test]
fn argv_omits_blackboard_for_ordinary_agents() {
    let args = run_args(&test_spec(false));
    assert!(
        !args.iter().any(|a| a.contains("blackboard")),
        "no blackboard mount for a non-workflow agent"
    );
    assert!(!values_of(&args, "-e").contains(&"WF_BLACKBOARD"));
}

/// Invariant 5: `~/.claude` is read-only so a prompt-injected agent cannot
/// plant a host-executed hook in `settings.json`, but `.credentials.json`
/// stays writable (appended *after* the RO dir mount) so token refresh
/// persists. No other read-write config surface may remain.
#[test]
fn argv_mounts_claude_readonly_with_writable_credentials() {
    let mut spec = test_spec(false);
    spec.mounts = claude_mounts(None, true, false);
    let args = run_args(&spec);
    let mounts = values_of(&args, "-v");

    // The dir is read-only; the credentials file is read-write on top.
    let dir_idx = mounts
        .iter()
        .position(|m| *m == "/Users/u/.claude:/Users/u/.claude:ro")
        .expect("~/.claude mounted read-only");
    let creds_idx = mounts
        .iter()
        .position(|m| {
            *m == "/Users/u/.claude/.credentials.json:/Users/u/.claude/.credentials.json"
        })
        .expect("credentials file mounted read-write");
    assert!(
        dir_idx < creds_idx,
        "RW credentials mount must follow the RO dir mount so Docker layers it on top",
    );

    // No read-write mount of any `~/.claude` path other than the credential
    // file — the whole point is that no config write surface survives.
    for mount in &mounts {
        let (src, _) = mount.split_once(':').unwrap();
        if src.starts_with("/Users/u/.claude") {
            assert!(
                mount.ends_with(":ro") || src.ends_with("/.credentials.json"),
                "unexpected read-write config surface: {mount}",
            );
        }
    }
}

/// Claude Code `mkdir`s `~/.claude/session-env/<id>` and writes a
/// `shell-snapshots/` entry every session; under the RO `~/.claude` mount
/// those fail with `EROFS` and abort the agent. Each gets an ephemeral
/// tmpfs overlay at its exact host path, ordered *after* the RO dir mount so
/// Docker layers it on top — and as `--tmpfs`, not `-v`, so no host write
/// surface is added (invariant 5).
#[test]
fn argv_overlays_ephemeral_runtime_dirs_with_tmpfs() {
    let args = run_args(&test_spec(false));

    // Exactly the whitelisted subdirs, at their identical host paths.
    assert_eq!(
        values_of(&args, "--tmpfs"),
        vec![
            "/Users/u/.claude/session-env",
            "/Users/u/.claude/shell-snapshots",
        ],
    );

    // The RO dir mount precedes every tmpfs overlay so Docker layers them on
    // top rather than under the read-only bind.
    let ro_idx = args
        .iter()
        .position(|a| a == "/Users/u/.claude:/Users/u/.claude:ro")
        .expect("~/.claude mounted read-only");
    for tmpfs in [
        "/Users/u/.claude/session-env",
        "/Users/u/.claude/shell-snapshots",
    ] {
        let idx = args.iter().position(|a| a == tmpfs).unwrap();
        assert!(
            ro_idx < idx,
            "tmpfs overlay {tmpfs} must follow the RO dir mount"
        );
    }

    // The overlays are tmpfs, never a `-v` bind — no `~/.claude` write
    // surface reaches the host.
    assert!(
        !values_of(&args, "-v")
            .iter()
            .any(|m| m.contains("/session-env") || m.contains("/shell-snapshots")),
        "runtime dirs must be tmpfs overlays, not host bind mounts",
    );
}

/// Claude persists its session transcript at `<config-dir>/projects/<slug>/
/// <uuid>.jsonl`; under the RO `~/.claude` mount that write fails, so
/// `--resume` can't survive a container recreation. A read-write bind of the
/// per-agent host dir (under `writable_root`) over `~/.claude/projects`
/// fixes it *without* exposing the shared `~/.claude/projects` — the bind
/// source is the isolated per-agent dir, not any host `~/.claude` path.
#[test]
fn argv_binds_per_agent_projects_dir_read_write() {
    let args = run_args(&test_spec(false));

    // The transcript overlay: per-agent host source → container projects/,
    // read-write (no `:ro` suffix).
    let overlay =
        "/Users/u/.fletch/worktrees/orkney/.fletch-claude-projects:/Users/u/.claude/projects";
    assert!(
        values_of(&args, "-v").contains(&overlay),
        "projects transcript overlay must be bound read-write",
    );

    // Ordered after the RO `~/.claude` mount so Docker layers it on top.
    let ro_idx = args
        .iter()
        .position(|a| a == "/Users/u/.claude:/Users/u/.claude:ro")
        .expect("~/.claude mounted read-only");
    let overlay_idx = args.iter().position(|a| a == overlay).unwrap();
    assert!(
        ro_idx < overlay_idx,
        "projects overlay must follow the RO dir mount"
    );

    // Invariant 5: no read-write bind draws from a host `~/.claude` path, so
    // the shared config's `projects/` (other agents' transcripts, global
    // memory) is unwritable. Only the read-only `~/.claude` bind may name it
    // as a source; this spec has no `.credentials.json`, its lone exception.
    for mount in values_of(&args, "-v") {
        if mount.ends_with(":ro") {
            continue;
        }
        let (src, _) = mount.split_once(':').unwrap();
        assert!(
            !src.starts_with("/Users/u/.claude"),
            "no host ~/.claude path may be a read-write bind source: {mount}",
        );
    }
}

/// A host with no credentials file still launches — the writable overlay is
/// skipped rather than pointing `-v` at a missing source (which Docker would
/// materialize as a root-owned directory).
#[test]
fn argv_omits_credentials_mount_when_file_absent() {
    let args = run_args(&test_spec(false)); // claude_credentials_rw: false
    assert!(
        !values_of(&args, "-v")
            .iter()
            .any(|m| m.contains("/.credentials.json")),
        "no credentials mount when the file is absent",
    );
}

#[test]
fn argv_mounts_borrowed_object_store_read_only() {
    // A --shared clone borrows the source's objects: the base three plus a
    // single RO mount of the borrowed store at its identical host path.
    let stores = vec![PathBuf::from("/Users/u/repo/.git/objects")];
    let mut spec = test_spec(false);
    spec.borrowed_object_stores = &stores;
    let args = run_args(&spec);
    // Order: workspace RW, mailbox RW, borrowed store RO, `~/.claude` RO
    // (invariant 5), then the RW per-agent `projects/` transcript overlay.
    assert_eq!(
        values_of(&args, "-v"),
        vec![
            "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
            "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
            "/Users/u/repo/.git/objects:/Users/u/repo/.git/objects:ro",
            "/Users/u/.claude:/Users/u/.claude:ro",
            "/Users/u/.fletch/worktrees/orkney/.fletch-claude-projects:/Users/u/.claude/projects",
        ],
    );
}

/// Codex mounts its config dir read-write (auth refresh + rollout writes
/// must reach the host) and launches the codex image + `codex` bin. There's
/// no `~/.claude` read-only mount, no tmpfs overlay, and no `projects/`
/// transcript bind — codex persists transcripts through this same RW mount.
#[test]
fn argv_codex_mounts_config_dir_read_write() {
    let args = run_args(&codex_spec());
    assert_eq!(
        values_of(&args, "-v"),
        vec![
            "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
            "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
            // ~/.codex read-write: no `:ro` suffix.
            "/Users/u/.codex:/Users/u/.codex",
        ],
    );
    // No claude-shaped surfaces leak into a codex launch.
    assert!(
        !args.iter().any(|a| a.contains("/.claude")),
        "codex must not mount any ~/.claude path"
    );
    assert!(
        values_of(&args, "--tmpfs").is_empty(),
        "codex has no tmpfs overlays"
    );
    assert!(
        !args.iter().any(|a| a.contains("fletch-claude-projects")),
        "codex has no projects/ transcript bind"
    );
    // Codex image + in-image bin, last (the prefix_args contract).
    assert_eq!(args[args.len() - 2], "fletch-agent-codex:abc123def456");
    assert_eq!(args[args.len() - 1], "codex");
}

/// Codex forwards `OPENAI_API_KEY` by bare name (invariant 3) and, unlike
/// claude, no `CLAUDE_CONFIG_DIR`/Anthropic vars. `CODEX_HOME` forwards only
/// for a non-default `$CODEX_HOME`.
#[test]
fn argv_codex_forwards_openai_key_and_optional_codex_home() {
    let args = run_args(&codex_spec());
    let forwarded = values_of(&args, "-e");
    assert!(
        forwarded.contains(&"OPENAI_API_KEY"),
        "missing bare -e OPENAI_API_KEY"
    );
    assert!(forwarded.contains(&"HOME"));
    assert!(!forwarded.contains(&"CLAUDE_CONFIG_DIR"));
    assert!(!forwarded.contains(&"ANTHROPIC_API_KEY"));
    // Default ~/.codex: CODEX_HOME is not forwarded (the mount + HOME cover it).
    assert!(
        !forwarded.contains(&"CODEX_HOME"),
        "default CODEX_HOME must not forward"
    );

    // A non-default $CODEX_HOME is forwarded so in-container codex reads it.
    let mut spec = codex_spec();
    spec.mounts = ProviderMounts::Codex {
        config_dir: Path::new("/Users/u/.codex"),
        forward_home: true,
    };
    assert!(values_of(&run_args(&spec), "-e").contains(&"CODEX_HOME"));

    // No token value anywhere in argv.
    for arg in &args {
        assert!(
            !arg.contains('=') || arg.starts_with("fletch."),
            "argv token `{arg}` carries a value — only label tokens may",
        );
    }
}

fn opencode_spec<'a>() -> RunSpec<'a> {
    rw_config_spec(
        ProviderMounts::Opencode {
            data_dir: Path::new("/Users/u/.local/share/opencode"),
            config_dir: None,
            forward_xdg_data_home: false,
            forward_xdg_config_home: false,
        },
        "fletch-agent-opencode:abc123def456",
        "opencode",
        &["ANTHROPIC_API_KEY"],
    )
}

fn pi_spec<'a>() -> RunSpec<'a> {
    rw_config_spec(
        ProviderMounts::Pi {
            data_dir: Path::new("/Users/u/.pi"),
        },
        "fletch-agent-pi:abc123def456",
        "pi",
        &["ANTHROPIC_API_KEY"],
    )
}

fn cursor_spec<'a>() -> RunSpec<'a> {
    rw_config_spec(
        ProviderMounts::Cursor {
            data_dir: Path::new("/Users/u/.cursor"),
        },
        "fletch-agent-cursor:abc123def456",
        "cursor-agent",
        &["CURSOR_API_KEY"],
    )
}

/// Assert no claude/codex config surface leaks into another provider's argv:
/// no `~/.claude` mount, no tmpfs overlay, no `projects/` transcript bind, and
/// no `~/.codex` mount. Shared by the opencode and pi mount tests.
fn assert_no_claude_or_codex_surface(args: &[String]) {
    assert!(
        !args.iter().any(|a| a.contains("/.claude")),
        "no ~/.claude path"
    );
    assert!(
        !args.iter().any(|a| a.contains("/.codex")),
        "no ~/.codex path"
    );
    assert!(values_of(args, "--tmpfs").is_empty(), "no tmpfs overlays");
    assert!(
        !args.iter().any(|a| a.contains("fletch-claude-projects")),
        "no projects/ transcript bind",
    );
}

/// OpenCode mounts its data dir read-write (accounts DB + session storage must
/// reach the host) and launches the opencode image + `opencode` bin. Its
/// config dir is absent here (the common case), so only the data dir mounts.
#[test]
fn argv_opencode_mounts_data_dir_read_write() {
    let args = run_args(&opencode_spec());
    assert_eq!(
        values_of(&args, "-v"),
        vec![
            "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
            "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
            // data dir read-write: no `:ro` suffix.
            "/Users/u/.local/share/opencode:/Users/u/.local/share/opencode",
        ],
    );
    assert_no_claude_or_codex_surface(&args);
    assert_eq!(args[args.len() - 2], "fletch-agent-opencode:abc123def456");
    assert_eq!(args[args.len() - 1], "opencode");
}

/// OpenCode's config dir (when present) mounts read-write after the data dir,
/// and a non-default XDG base forwards the matching var by bare name only.
#[test]
fn argv_opencode_mounts_config_dir_and_forwards_xdg() {
    let mut spec = opencode_spec();
    spec.mounts = ProviderMounts::Opencode {
        data_dir: Path::new("/xdg/data/opencode"),
        config_dir: Some(Path::new("/Users/u/.config/opencode")),
        forward_xdg_data_home: true,
        forward_xdg_config_home: false,
    };
    let args = run_args(&spec);
    let mounts = values_of(&args, "-v");
    assert!(mounts.contains(&"/xdg/data/opencode:/xdg/data/opencode"));
    assert!(mounts.contains(&"/Users/u/.config/opencode:/Users/u/.config/opencode"));

    let forwarded = values_of(&args, "-e");
    assert!(
        forwarded.contains(&"XDG_DATA_HOME"),
        "non-default XDG_DATA_HOME forwards"
    );
    assert!(
        !forwarded.contains(&"XDG_CONFIG_HOME"),
        "default XDG_CONFIG_HOME must not forward"
    );
    assert!(forwarded.contains(&"ANTHROPIC_API_KEY"));
    // No value token in argv (invariant 3).
    for arg in &args {
        assert!(
            !arg.contains('=') || arg.starts_with("fletch."),
            "argv token `{arg}` carries a value",
        );
    }
}

/// Pi mounts `~/.pi` read-write and launches the pi image + `pi` bin; no
/// claude/codex surface, and the forwarded key rides by bare name only.
#[test]
fn argv_pi_mounts_dot_pi_read_write() {
    let args = run_args(&pi_spec());
    assert_eq!(
        values_of(&args, "-v"),
        vec![
            "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
            "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
            "/Users/u/.pi:/Users/u/.pi",
        ],
    );
    assert_no_claude_or_codex_surface(&args);
    let forwarded = values_of(&args, "-e");
    assert!(
        forwarded.contains(&"ANTHROPIC_API_KEY"),
        "missing bare -e ANTHROPIC_API_KEY"
    );
    assert!(!forwarded.contains(&"CLAUDE_CONFIG_DIR"));
    assert!(!forwarded.contains(&"CODEX_HOME"));
    for arg in &args {
        assert!(
            !arg.contains('=') || arg.starts_with("fletch."),
            "argv token `{arg}` carries a value",
        );
    }
    assert_eq!(args[args.len() - 2], "fletch-agent-pi:abc123def456");
    assert_eq!(args[args.len() - 1], "pi");
}

/// Cursor mounts `~/.cursor` read-write (session transcripts must reach the
/// host) and launches the cursor image + `cursor-agent` bin; no claude/codex
/// surface, and its sole auth var rides by bare name only (invariant 3).
#[test]
fn argv_cursor_mounts_dot_cursor_read_write() {
    let args = run_args(&cursor_spec());
    assert_eq!(
        values_of(&args, "-v"),
        vec![
            "/Users/u/.fletch/worktrees/orkney:/Users/u/.fletch/worktrees/orkney",
            "/Users/u/.fletch/rpc/orkney:/Users/u/.fletch/rpc/orkney",
            "/Users/u/.cursor:/Users/u/.cursor",
        ],
    );
    assert_no_claude_or_codex_surface(&args);
    let forwarded = values_of(&args, "-e");
    assert!(
        forwarded.contains(&"CURSOR_API_KEY"),
        "missing bare -e CURSOR_API_KEY"
    );
    assert!(!forwarded.contains(&"CLAUDE_CONFIG_DIR"));
    assert!(!forwarded.contains(&"CODEX_HOME"));
    // No token value in argv, only the label token may carry an `=`.
    for arg in &args {
        assert!(
            !arg.contains('=') || arg.starts_with("fletch."),
            "argv token `{arg}` carries a value",
        );
    }
    assert_eq!(args[args.len() - 2], "fletch-agent-cursor:abc123def456");
    assert_eq!(args[args.len() - 1], "cursor-agent");
}

#[test]
fn argv_mounts_every_chained_alternate_read_only() {
    // Every store the workspace borrows (directly or transitively) gets its
    // own RO mount; `run_args` mounts whatever `borrowed_object_stores`
    // resolved.
    let stores = vec![
        PathBuf::from("/Users/u/repo/.git/objects"),
        PathBuf::from("/Users/u/shared-cache/objects"),
    ];
    let mut spec = test_spec(false);
    spec.borrowed_object_stores = &stores;
    let args = run_args(&spec);
    // Object-store RO mounts only (exclude the `~/.claude:ro` mount, which
    // also ends in `:ro` under invariant 5).
    let ro: Vec<&str> = values_of(&args, "-v")
        .into_iter()
        .filter(|m| m.ends_with(":ro") && m.contains("/objects"))
        .collect();
    assert_eq!(
        ro,
        vec![
            "/Users/u/repo/.git/objects:/Users/u/repo/.git/objects:ro",
            "/Users/u/shared-cache/objects:/Users/u/shared-cache/objects:ro",
        ],
    );
}

#[test]
fn borrowed_object_stores_reads_alternates_lines() {
    // Layout: writable_root/<subdir>/.git/objects/info/alternates.
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    let info = root.join("repo/.git/objects/info");
    std::fs::create_dir_all(&info).unwrap();

    // Absent alternates → nothing to mount (worktree / full-copy clone).
    assert!(borrowed_object_stores(root).is_empty());

    std::fs::write(
        info.join("alternates"),
        "/src/a/.git/objects\n\n  /src/b/objects  \n",
    )
    .unwrap();
    assert_eq!(
        borrowed_object_stores(root),
        vec![
            PathBuf::from("/src/a/.git/objects"),
            PathBuf::from("/src/b/objects"),
        ],
    );
}

#[test]
fn borrowed_object_stores_follows_chained_alternates() {
    // Model checkout --shared→ B --shared→ A: the checkout points only at
    // B; B points at A. Both B and A must be discovered so both get
    // mounted, or in-container git can't reach A's objects.
    let td = tempfile::tempdir().unwrap();
    let a = td.path().join("A/.git/objects");
    let b = td.path().join("B/.git/objects");
    // The checkout lives under the writable root as a subdir.
    let checkout = td.path().join("root/repo/.git/objects");
    for dir in [&a, &b, &checkout] {
        std::fs::create_dir_all(dir.join("info")).unwrap();
    }
    std::fs::write(
        checkout.join("info/alternates"),
        format!("{}\n", b.display()),
    )
    .unwrap();
    std::fs::write(b.join("info/alternates"), format!("{}\n", a.display())).unwrap();

    assert_eq!(
        borrowed_object_stores(&td.path().join("root")),
        vec![b, a],
        "chain must resolve checkout→B→A, mounting both borrowed stores"
    );
}

#[test]
fn borrowed_object_stores_scans_every_repo_checkout() {
    // A multi-repo agent: two shared-clone checkouts under one writable
    // root, each borrowing a different source. Both borrowed stores must be
    // discovered — scanning only the primary would strand the secondary.
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("agent");
    let primary = root.join("app/.git/objects/info");
    let secondary = root.join("lib/.git/objects/info");
    std::fs::create_dir_all(&primary).unwrap();
    std::fs::create_dir_all(&secondary).unwrap();
    std::fs::write(primary.join("alternates"), "/src/app/.git/objects\n").unwrap();
    std::fs::write(secondary.join("alternates"), "/src/lib/.git/objects\n").unwrap();

    // Sorted by subdir name: `app` before `lib`.
    assert_eq!(
        borrowed_object_stores(&root),
        vec![
            PathBuf::from("/src/app/.git/objects"),
            PathBuf::from("/src/lib/.git/objects"),
        ],
    );
}

#[test]
fn argv_shape_and_pid1_flags() {
    let args = run_args(&test_spec(false));
    assert_eq!(args[0], "run");
    assert!(args.contains(&"--rm".to_string()));
    assert!(args.contains(&"--init".to_string()));
    assert!(args.contains(&"-i".to_string()));
    assert!(
        !args.contains(&"-t".to_string()),
        "stdio launch must not allocate a tty"
    );
    // prefix_args contract: image then agent bin, last — the caller
    // appends agent CLI args directly after.
    assert_eq!(args[args.len() - 2], "fletch-agent:abc123def456");
    assert_eq!(args[args.len() - 1], "claude");
    assert_eq!(values_of(&args, "--name"), vec!["fletch-orkney-deadbeef"]);
    assert_eq!(values_of(&args, "--memory"), vec!["4g"]);
    assert_eq!(values_of(&args, "--cpus"), vec!["2"]);

    let interactive = run_args(&test_spec(true));
    assert!(
        interactive.contains(&"-t".to_string()),
        "pty launch gets a tty"
    );
}

#[test]
fn argv_labels_carry_pid_and_agent_id() {
    let args = run_args(&test_spec(false));
    let labels = values_of(&args, "--label");
    assert!(labels.contains(&format!("fletch.host-pid={}", std::process::id()).as_str()));
    assert!(labels.contains(&"fletch.agent-id=orkney"));
}

/// Invariant 3: auth is forwarded by bare name only — no token value can
/// ever appear in argv, whatever the environment holds.
#[test]
fn argv_forwards_auth_by_bare_name_never_value() {
    let args = run_args(&test_spec(false));
    let forwarded = values_of(&args, "-e");
    for var in [
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
    ] {
        assert!(forwarded.contains(&var), "missing bare -e {var}");
    }
    for arg in &args {
        assert!(
            !arg.contains('=') || arg.starts_with("fletch."),
            "argv token `{arg}` carries a value — only label tokens may",
        );
    }
    // Non-auth runtime env is forwarded the same way.
    for var in ["HOME", "FLETCH_RPC_DIR", "TERM", "COLORTERM"] {
        assert!(forwarded.contains(&var), "missing bare -e {var}");
    }
    assert!(
        !forwarded.contains(&"CLAUDE_CONFIG_DIR"),
        "default config dir must not be forwarded",
    );
}

#[test]
fn argv_mounts_and_forwards_nondefault_claude_config_dir() {
    // The non-default config dir gets the same read-only-except-credentials
    // treatment as `~/.claude` (invariant 5).
    let mut spec = test_spec(false);
    spec.mounts = claude_mounts(Some(Path::new("/Users/u/.claude-eve")), false, true);
    let args = run_args(&spec);
    let mounts = values_of(&args, "-v");
    assert!(mounts.contains(&"/Users/u/.claude-eve:/Users/u/.claude-eve:ro"));
    assert!(mounts.contains(
        &"/Users/u/.claude-eve/.credentials.json:/Users/u/.claude-eve/.credentials.json"
    ));
    assert!(values_of(&args, "-e").contains(&"CLAUDE_CONFIG_DIR"));
}

#[test]
fn config_dir_is_default_canonicalizes_both_sides() {
    let td = tempfile::tempdir().unwrap();
    let home = td.path();
    let default = home.join(".claude");
    std::fs::create_dir_all(&default).unwrap();

    // The literal default, and its trailing-slash spelling, are default.
    assert!(config_dir_is_default(&default, home));
    assert!(config_dir_is_default(&home.join(".claude/"), home));
    // A genuinely different dir is not.
    assert!(!config_dir_is_default(&home.join(".claude-eve"), home));

    // A symlink that resolves to the default is treated as default — the
    // both-sides canonicalization this test exists to prove. (On macOS the
    // tempdir itself lives under a `/var`→`/private/var` symlink, so the
    // home side is exercised too.)
    #[cfg(unix)]
    {
        let link = home.join("link-to-claude");
        std::os::unix::fs::symlink(&default, &link).unwrap();
        assert!(
            config_dir_is_default(&link, home),
            "a symlink resolving to ~/.claude must read as default"
        );
    }
}

#[test]
fn container_name_shape_and_nonce_uniqueness() {
    let a = container_name("orkney");
    let b = container_name("orkney");
    for name in [&a, &b] {
        let nonce = name.strip_prefix("fletch-orkney-").expect("prefix");
        assert_eq!(nonce.len(), 8);
        assert!(nonce.chars().all(|c| c.is_ascii_hexdigit()));
    }
    assert_ne!(a, b, "respawns must never reuse a container name");

    // Ids are word-safe today; anything unexpected sanitizes to '-'.
    assert!(container_name("we ird/id").starts_with("fletch-we-ird-id-"));
}

#[test]
fn exit_code_mapping_is_distinct_and_scoped() {
    let daemon = describe_exit_code(125).unwrap();
    let not_exec = describe_exit_code(126).unwrap();
    let missing = describe_exit_code(127).unwrap();
    assert!(daemon.contains("daemon"), "{daemon}");
    assert!(not_exec.contains("not runnable"), "{not_exec}");
    assert!(missing.contains("no agent binary"), "{missing}");
    let distinct: std::collections::HashSet<_> = [&daemon, &not_exec, &missing].into();
    assert_eq!(distinct.len(), 3);
    // Each hedges: docker relays the agent's own status, so these codes can
    // originate inside the container — the message must not over-claim.
    for msg in [&daemon, &not_exec, &missing] {
        assert!(msg.contains("agent itself exited"), "must hedge: {msg}");
    }
    for code in [0, 1, 2, 124, 128, 130, 137, 143] {
        assert_eq!(
            describe_exit_code(code),
            None,
            "code {code} must pass through"
        );
    }
}

#[test]
fn blank_settings_fall_back_to_defaults() {
    assert_eq!(non_blank(None), None);
    assert_eq!(non_blank(Some("")), None);
    assert_eq!(non_blank(Some("  ")), None);
    assert_eq!(non_blank(Some(" 8g ")), Some("8g"));
}

/// Happy path: a resolved auth env lands on the docker CLI process env
/// verbatim, and forwarding exactly those names puts a matching bare
/// `-e NAME` in argv for each — so values forward into the container yet
/// never appear in argv (invariant 3).
#[test]
fn resolved_auth_forwards_values_in_env_never_argv() {
    use super::super::auth::AuthSource;

    let secret = "sk-ant-oat-SECRET-VALUE";
    let resolved = ContainerAuth::Resolved {
        env: vec![
            ("CLAUDE_CODE_OAUTH_TOKEN".to_string(), secret.to_string()),
            (
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                "proxy-secret".to_string(),
            ),
        ],
        source: AuthSource::StoredToken,
    };
    let mut env: Vec<(String, String)> = Vec::new();
    apply_container_auth(&mut env, resolved).expect("resolved auth applies");

    // Values ride the CLI process env.
    assert!(env
        .iter()
        .any(|(k, v)| k == "CLAUDE_CODE_OAUTH_TOKEN" && v == secret));
    assert!(env
        .iter()
        .any(|(k, v)| k == "ANTHROPIC_AUTH_TOKEN" && v == "proxy-secret"));

    // Forwarding exactly those names emits a bare `-e NAME` for each, with
    // no value (secret or otherwise) anywhere in argv.
    let auth_var_names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    let mut spec = test_spec(false);
    spec.auth_vars = &auth_var_names;
    let args = run_args(&spec);
    let forwarded = values_of(&args, "-e");
    for (name, _) in &env {
        assert!(
            forwarded.contains(&name.as_str()),
            "resolved var {name} has no bare -e in argv",
        );
    }
    for arg in &args {
        assert!(!arg.contains(secret), "secret leaked into argv: {arg}");
        assert!(!arg.contains("proxy-secret"), "proxy secret in argv: {arg}");
    }
}

/// D1 swap, empty resolution (`CredentialsFile`): no env additions, no
/// error — the `~/.claude` mount carries the credential.
#[test]
fn resolved_auth_with_empty_env_is_a_noop() {
    use super::super::auth::AuthSource;

    let mut env: Vec<(String, String)> = Vec::new();
    apply_container_auth(
        &mut env,
        ContainerAuth::Resolved {
            env: Vec::new(),
            source: AuthSource::CredentialsFile,
        },
    )
    .expect("credentials-file resolves");
    assert!(env.is_empty());
}

/// D1 swap, `Unavailable`: the launch fails fast with the settings pointer
/// C2 keys its call-to-action on. Asserts the stable substrings so the
/// wording can evolve without silently breaking the UI match.
#[test]
fn unavailable_auth_fails_launch_with_settings_pointer() {
    let mut env: Vec<(String, String)> = Vec::new();
    let err = apply_container_auth(&mut env, ContainerAuth::Unavailable)
        .expect_err("Unavailable must block the launch");
    let msg = err.to_string();
    assert!(msg.contains("Settings"), "no settings pointer: {msg}");
    assert!(msg.contains("setup-token"), "no setup-token hint: {msg}");
    assert_eq!(msg, NO_CONTAINER_AUTH_MSG);
    assert!(env.is_empty(), "a failed resolution must add no env");
}

/// Codex auth: a non-blank `OPENAI_API_KEY` is forwarded (trimmed); a bare
/// `auth.json` resolves with nothing to inject (the mount carries it); a
/// blank key falls back to the file; neither present fails the launch.
#[test]
fn codex_auth_env_resolves_key_file_or_fails() {
    // API key wins and is trimmed.
    assert_eq!(
        codex_auth_env(Some(" sk-openai \n"), false).unwrap(),
        vec![("OPENAI_API_KEY".to_string(), "sk-openai".to_string())],
    );
    // Key alongside a file still forwards the key.
    assert_eq!(
        codex_auth_env(Some("sk-openai"), true).unwrap(),
        vec![("OPENAI_API_KEY".to_string(), "sk-openai".to_string())],
    );
    // No key but a mounted auth.json: nothing to inject, but resolves.
    assert!(codex_auth_env(None, true).unwrap().is_empty());
    assert!(codex_auth_env(Some("   "), true).unwrap().is_empty());
    // Neither: fail-fast with the settings pointer.
    let err = codex_auth_env(None, false).unwrap_err().to_string();
    assert_eq!(err, NO_CODEX_AUTH_MSG);
    assert!(codex_auth_env(Some("  "), false).is_err());
}

/// Regression: a key-only user (`OPENAI_API_KEY` set, never ran codex on
/// the host) must launch — the missing config dir is created for the RW
/// mount, not treated as "no way to authenticate". With no credential at
/// all the launch still fails, and before touching the filesystem.
#[test]
fn codex_key_only_launch_creates_missing_config_dir() {
    let td = tempfile::tempdir().unwrap();

    let dir = td.path().join(".codex");
    let mut env = Vec::new();
    prepare_codex_launch(&mut env, &dir, Some("sk-openai")).unwrap();
    assert!(dir.is_dir(), "config dir must exist for the RW bind mount");
    assert_eq!(
        env,
        vec![("OPENAI_API_KEY".to_string(), "sk-openai".to_string())],
    );

    let no_auth = td.path().join(".codex-no-auth");
    let err = prepare_codex_launch(&mut Vec::new(), &no_auth, None).unwrap_err();
    assert_eq!(err.to_string(), NO_CODEX_AUTH_MSG);
    assert!(!no_auth.exists(), "auth failure must not create the dir");
}

/// `present_api_keys` returns only the known, non-blank vars, trimmed, in the
/// constant's order — and never a var outside the curated set.
#[test]
fn present_api_keys_filters_trims_and_orders() {
    let env: std::collections::HashMap<&str, &str> = [
        ("OPENAI_API_KEY", " sk-openai \n"),
        ("ANTHROPIC_API_KEY", "sk-ant"),
        ("GROQ_API_KEY", "   "),    // blank → dropped
        ("SOME_OTHER_KEY", "nope"), // not in the curated set → dropped
    ]
    .into_iter()
    .collect();
    let keys = present_api_keys(|n| env.get(n).map(|v| v.to_string()));
    assert_eq!(
        keys,
        vec![
            // ANTHROPIC precedes OPENAI in MULTI_PROVIDER_API_KEY_ENV.
            ("ANTHROPIC_API_KEY".to_string(), "sk-ant".to_string()),
            ("OPENAI_API_KEY".to_string(), "sk-openai".to_string()),
        ],
    );
    assert!(present_api_keys(|_| None).is_empty());
}

/// The shared multi-provider rule: a forwarded key OR a credential on the
/// mount resolves (returning the keys to forward); neither → the given error.
#[test]
fn multi_provider_auth_env_key_or_mount_or_fail() {
    let key = vec![("ANTHROPIC_API_KEY".to_string(), "sk".to_string())];
    // Key present: forwarded, regardless of the mount.
    assert_eq!(
        multi_provider_auth_env(key.clone(), false, NO_OPENCODE_AUTH_MSG).unwrap(),
        key,
    );
    // No key but a credential on the mount: resolves with nothing to inject.
    assert!(multi_provider_auth_env(Vec::new(), true, NO_PI_AUTH_MSG)
        .unwrap()
        .is_empty());
    // Neither: the caller's fail-fast message.
    let err = multi_provider_auth_env(Vec::new(), false, NO_OPENCODE_AUTH_MSG).unwrap_err();
    assert_eq!(err.to_string(), NO_OPENCODE_AUTH_MSG);
}

/// Regression (mirrors the codex key-only case): an opencode user with only a
/// provider API key set (never ran opencode) still launches — the missing data
/// dir is created for the RW bind. A stored login on the mount (opencode.db or
/// auth.json) also resolves. No credential at all fails before touching disk.
#[test]
fn opencode_key_only_launch_creates_missing_data_dir() {
    let td = tempfile::tempdir().unwrap();

    // Key-only: dir created, key forwarded.
    let dir = td.path().join("share/opencode");
    let mut env = Vec::new();
    let keys = vec![("OPENAI_API_KEY".to_string(), "sk".to_string())];
    prepare_opencode_launch(&mut env, &dir, keys.clone()).unwrap();
    assert!(dir.is_dir(), "data dir must exist for the RW bind mount");
    assert_eq!(env, keys);

    // Stored login on the mount (opencode.db), no key: resolves, nothing to inject.
    let db_dir = td.path().join("with-db/opencode");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::write(db_dir.join("opencode.db"), b"x").unwrap();
    let mut env2 = Vec::new();
    prepare_opencode_launch(&mut env2, &db_dir, Vec::new()).unwrap();
    assert!(env2.is_empty());

    // Neither: fail fast, no dir created.
    let none = td.path().join("no-auth/opencode");
    let err = prepare_opencode_launch(&mut Vec::new(), &none, Vec::new()).unwrap_err();
    assert_eq!(err.to_string(), NO_OPENCODE_AUTH_MSG);
    assert!(!none.exists(), "auth failure must not create the dir");
}

/// Regression for pi: key-only user launches (`~/.pi` created for the RW bind);
/// `~/.pi/agent/auth.json` on the mount also resolves; neither fails fast.
#[test]
fn pi_key_only_launch_creates_missing_data_dir() {
    let td = tempfile::tempdir().unwrap();

    let dir = td.path().join(".pi");
    let mut env = Vec::new();
    let keys = vec![("ANTHROPIC_API_KEY".to_string(), "sk".to_string())];
    prepare_pi_launch(&mut env, &dir, keys.clone()).unwrap();
    assert!(dir.is_dir(), "~/.pi must exist for the RW bind mount");
    assert_eq!(env, keys);

    // auth.json on the mount, no key: resolves, nothing to inject.
    let with_auth = td.path().join(".pi-authed");
    std::fs::create_dir_all(with_auth.join("agent")).unwrap();
    std::fs::write(with_auth.join("agent/auth.json"), b"{}").unwrap();
    let mut env2 = Vec::new();
    prepare_pi_launch(&mut env2, &with_auth, Vec::new()).unwrap();
    assert!(env2.is_empty());

    let none = td.path().join(".pi-no-auth");
    let err = prepare_pi_launch(&mut Vec::new(), &none, Vec::new()).unwrap_err();
    assert_eq!(err.to_string(), NO_PI_AUTH_MSG);
    assert!(!none.exists(), "auth failure must not create the dir");
}

/// Cursor auth: a non-blank `CURSOR_API_KEY` is forwarded (trimmed); anything
/// else fails the launch. Unlike the other providers there is *no* mount
/// fallback — the keychain-bound login token can't reach a container — so a
/// missing/blank key is the only outcome besides a forwarded key.
#[test]
fn cursor_auth_env_forwards_key_or_fails() {
    assert_eq!(
        cursor_auth_env(Some(" cur-key \n")).unwrap(),
        vec![("CURSOR_API_KEY".to_string(), "cur-key".to_string())],
    );
    // No mount fallback: unset and blank both fail with the settings pointer.
    assert_eq!(
        cursor_auth_env(None).unwrap_err().to_string(),
        NO_CURSOR_AUTH_MSG
    );
    assert_eq!(
        cursor_auth_env(Some("   ")).unwrap_err().to_string(),
        NO_CURSOR_AUTH_MSG
    );
}

/// Regression (mirrors the codex key-only case): a cursor user with
/// `CURSOR_API_KEY` set launches — the missing `~/.cursor` is created for the
/// RW transcript bind. With no key the launch fails, before touching disk
/// (there is no mounted-credential path for cursor to fall back to).
#[test]
fn cursor_key_only_launch_creates_missing_config_dir() {
    let td = tempfile::tempdir().unwrap();

    let dir = td.path().join(".cursor");
    let mut env = Vec::new();
    prepare_cursor_launch(&mut env, &dir, Some("cur-key")).unwrap();
    assert!(dir.is_dir(), "~/.cursor must exist for the RW bind mount");
    assert_eq!(
        env,
        vec![("CURSOR_API_KEY".to_string(), "cur-key".to_string())]
    );

    let no_auth = td.path().join(".cursor-no-auth");
    let err = prepare_cursor_launch(&mut Vec::new(), &no_auth, None).unwrap_err();
    assert_eq!(err.to_string(), NO_CURSOR_AUTH_MSG);
    assert!(!no_auth.exists(), "auth failure must not create the dir");
}

/// Integration: a real `docker run` round-trip through the exact argv the
/// engine builds — busybox standing in for the agent image, `echo` for
/// the agent binary. `FLETCH_DOCKER_TESTS=1 cargo test -- --ignored`
#[test]
#[ignore = "requires Docker; opt in via FLETCH_DOCKER_TESTS=1"]
fn docker_run_echo_round_trip() {
    if !crate::sandbox::docker::docker_tests_enabled() {
        return;
    }
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("root");
    let rpc = td.path().join("rpc");
    let home = td.path().join("home");
    for d in [&root, &rpc] {
        std::fs::create_dir_all(d).unwrap();
    }
    // Same mount-source/mountpoint prep `launch_agent` does, so the tmpfs
    // overlays and the `projects/` bind have targets under the RO `~/.claude`
    // bind and a source dir that isn't materialized root-owned.
    prepare_config_mount_dir(&home.join(".claude")).unwrap();
    let projects_src = root.join(crate::transcripts::DOCKER_CLAUDE_PROJECTS_DIRNAME);
    std::fs::create_dir_all(&projects_src).unwrap();
    let name = container_name("b2-int-test");
    let args = run_args(&RunSpec {
        interactive: false,
        name: &name,
        agent_id: "b2-int-test",
        writable_root: &root,
        rpc_dir: &rpc,
        home: &home,
        cwd: &root,
        blackboard: None,
        mounts: ProviderMounts::Claude {
            config_dir: None,
            credentials_rw: false,
            config_dir_credentials_rw: false,
            projects_src: &projects_src,
        },
        borrowed_object_stores: &[],
        memory: "256m",
        cpus: "1",
        image: "busybox",
        agent_bin: "echo",
        auth_vars: &[],
    });
    let docker = cli::docker_bin().expect("docker installed");
    let out = std::process::Command::new(docker)
        .args(&args)
        .arg("hello-from-container")
        .env("HOME", &home)
        .env("FLETCH_RPC_DIR", &rpc)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "docker run failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "hello-from-container",
    );
}

/// Integration: kill and liveness against a live container.
/// `FLETCH_DOCKER_TESTS=1 cargo test -- --ignored`
#[test]
#[ignore = "requires Docker; opt in via FLETCH_DOCKER_TESTS=1"]
fn kill_and_liveness_against_live_container() {
    if !crate::sandbox::docker::docker_tests_enabled() {
        return;
    }
    let name = container_name("b2-kill-test");
    let out = cli::run_docker(
        &[
            "run", "-d", "--rm", "--name", &name, "busybox", "sleep", "60",
        ],
        Duration::from_secs(60),
    )
    .unwrap();
    assert!(
        out.status.success(),
        "docker run failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );

    let engine = DockerEngine::shared();
    let plan = KillPlan::Container { name: name.clone() };
    assert!(
        container_running(&name),
        "fresh container should be running"
    );
    engine.kill(&plan).unwrap();
    assert!(!container_running(&name), "killed container reads as dead");
}
