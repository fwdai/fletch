//! End to end against a real box. Ignored by default; needs the dev runtime
//! and a built image:
//!
//! ```sh
//! eval "$(scripts/box-dev.sh)"
//! FLETCH_BOX_IMAGE_DIR=$PWD/packaging/box-image/out \
//!   cargo test -p fletch-core boxes::tests -- --ignored --nocapture --test-threads 1
//! ```
//!
//! `FLETCH_BOX_IMAGE_DIR` is the directory holding `base.erofs` (what the
//! `box_image_url` setting would name as `file://…`); the test installs it into
//! a temporary data dir. A tools disk is built unless `FLETCH_BOX_TOOLS_DISK`
//! names an existing one to attach instead (a build takes minutes); one named
//! `…+codegraph-<tag>.img` is taken as carrying codegraph.
//!
//! The runtime is multi-threaded on purpose: the sync seams boot a box from a
//! scoped thread with `Handle::block_on`, which a current-thread runtime
//! cannot drive.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::block_on_with;
use super::provision::Landing;
use super::registry::{BoxIdentity, BoxRegistry, StopReason};
use super::settings::{self, BoxSettings};
use crate::error::Error;
use crate::sandbox::engine::AgentLaunchCtx;
use crate::sandbox::policy;
use crate::sandbox::provision::CheckoutSpec;
use crate::workspace::FolderGrant;

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} must be set")))
}

/// Points the engine at a temp data dir, installs the image from
/// `FLETCH_BOX_IMAGE_DIR`. Returns the data dir guard.
async fn setup() -> tempfile::TempDir {
    let data = tempfile::tempdir().unwrap();
    crate::sandbox::set_data_dir(data.path().to_path_buf());
    let image_dir = env_path("FLETCH_BOX_IMAGE_DIR");
    settings::set(BoxSettings {
        image_url: Some(format!("file://{}", image_dir.display())),
        idle_stop: Some(Duration::from_secs(600)),
        ..BoxSettings::default()
    });
    let t0 = Instant::now();
    let installed = super::image_store()
        .install(&super::pinned_image(), &super::image_layers(), |_| {
            std::ops::ControlFlow::Continue(())
        })
        .await
        .unwrap_or_else(|e| panic!("install the image: {e}"));
    eprintln!(
        "image installed ({}) in {:.2?}",
        installed.version,
        t0.elapsed()
    );
    if let Some(disk) = std::env::var_os("FLETCH_BOX_TOOLS_DISK") {
        // Its name says whether it carries codegraph (`fletch_box::tools::
        // disk_path`). A plain one with indexing on would have the first boot
        // build the codegraph disk beside the test; the point of passing a
        // disk is to build none.
        let codegraph = fletch_box::tools::carries_codegraph(Path::new(&disk));
        if !codegraph {
            crate::codegraph::set_enabled(false);
        }
        let target = super::registry::tools_disk_path("claude", codegraph);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        fletch_box::disks::clone(Path::new(&disk), &target).unwrap();
    }
    data
}

fn identity(agent_id: &str) -> BoxIdentity {
    BoxIdentity::new(agent_id, "box-e2e-project", "claude").unwrap()
}

/// Boot → `uname -r` → provision this repository's HEAD into the guest →
/// `git_state::query` through the seam → write a file through the seam and
/// see it in `git status` → one mailbox request round trip → clean stop with
/// no `dirty` → reboot timing → discard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "boots a real box; see the module docs"]
async fn a_box_runs_the_whole_workspace_contract() {
    let _data = setup().await;
    let agent_id = format!("e2e-{}", std::process::id() % 10_000);
    let identity = identity(&agent_id);
    let registry = BoxRegistry::global();
    std::fs::create_dir_all(&identity.writable_root).unwrap();

    // Boot. Without `FLETCH_BOX_TOOLS_DISK` this first builds the tools disk
    // (minutes), as the engine does for a launch during setup.
    let t0 = Instant::now();
    let running = registry
        .ensure_running(&identity)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let booted = t0.elapsed();
    eprintln!(
        "first boot ready in {booted:.2?} (includes hashing the image once per process; \
         tools disks attached: {:?})",
        running.tools_providers()
    );

    let uname = running
        .channel
        .exec(None, None, &["uname", "-r"])
        .await
        .unwrap();
    assert!(uname.status.success());
    eprintln!(
        "guest kernel: {}",
        String::from_utf8_lossy(&uname.stdout).trim()
    );

    // Provision this repository's HEAD into the guest.
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let head = crate::git::rev_parse(&repo, "HEAD").await.unwrap();
    let dest = identity.writable_root.join("fletch");
    let spec = CheckoutSpec {
        source_repo: &repo,
        base_ref: &head,
        dest: &dest,
    };
    let t1 = Instant::now();
    let freshness = super::provision::provision(&identity, &spec, None, Landing::Detached)
        .await
        .unwrap_or_else(|e| panic!("provision: {e}"));
    eprintln!(
        "provisioned HEAD into the guest in {:.2?} ({freshness:?})",
        t1.elapsed()
    );

    // The routed seam: git in the guest.
    let t2 = Instant::now();
    let guest_head = crate::git::rev_parse(&dest, "HEAD").await.unwrap();
    assert_eq!(guest_head, head);
    // Resolved in the guest like the Git panel does it: the base is the
    // repository's own current branch, pushed in as `origin/<branch>`.
    let branch = crate::git::default_branch(&repo).await;
    let base = crate::git::resolve_base(&dest, &repo, &branch, Some(&head)).await;
    let state = crate::git_state::query(&dest, &base).await.unwrap();
    assert!(
        state.files.is_empty(),
        "a fresh checkout is clean: {:?}",
        state.files
    );
    eprintln!("git_state::query through the seam in {:.2?}", t2.elapsed());

    // A write through the seam shows up in the guest's `git status`.
    let exec = crate::exec::for_path(&dest);
    assert!(matches!(exec, crate::exec::Executor::Remote(_)));
    exec.write(&dest.join("BOX_E2E.md"), b"written from the host\n")
        .unwrap();
    let state = crate::git_state::query(&dest, &base).await.unwrap();
    assert!(
        state.files.iter().any(|f| f.path == "BOX_E2E.md"),
        "{:?}",
        state.files
    );
    assert_eq!(
        exec.read(&dest.join("BOX_E2E.md")).unwrap(),
        b"written from the host\n"
    );
    let listed = exec.list(&dest).unwrap();
    assert!(listed.iter().any(|e| e.name == "BOX_E2E.md" && !e.is_dir));
    assert!(listed.iter().any(|e| e.name == "crates" && e.is_dir));

    // Mailbox: a request the agent writes in the guest reaches the host
    // mailbox; a response written there reaches the guest.
    let t3 = Instant::now();
    let guest_requests = identity.rpc_dir.join("requests");
    let request = r#"{"id":"e2e-1","op":"noop","args":{}}"#;
    super::executor::put(
        &running.channel,
        request.as_bytes(),
        &guest_requests.join("e2e-1.json"),
        0o600,
    )
    .unwrap();
    let host_request = identity.rpc_dir.join("requests/e2e-1.json");
    wait_for(|| host_request.exists(), Duration::from_secs(5)).await;
    assert_eq!(std::fs::read_to_string(&host_request).unwrap(), request);
    std::fs::remove_file(&host_request).unwrap();
    std::fs::write(
        identity.rpc_dir.join("responses/e2e-1.json"),
        r#"{"id":"e2e-1","ok":true}"#,
    )
    .unwrap();
    let guest_response = identity.rpc_dir.join("responses/e2e-1.json");
    let channel = running.channel.clone();
    wait_for(
        || {
            super::executor::run(
                &channel,
                &["test", "-f", &guest_response.to_string_lossy()],
                None,
            )
            .is_ok_and(|out| out.status.success())
        },
        Duration::from_secs(5),
    )
    .await;
    eprintln!("mailbox round trip in {:.2?}", t3.elapsed());

    // Clean stop: Control, no dirty marker, mirror refreshed.
    let t4 = Instant::now();
    let outcome = registry.stop(&agent_id, StopReason::User).await.unwrap();
    eprintln!("stopped via {outcome:?} in {:.2?}", t4.elapsed());
    assert_eq!(outcome, fletch_box::StopOutcome::Control);
    let box_dir = super::box_dir(&agent_id).unwrap();
    assert!(!fletch_box::disks::Dirty::in_dir(&box_dir).is_set());
    let mirror = super::mirror::mirror_dir(&box_dir);
    assert!(
        mirror.join("HEAD").exists(),
        "the stop refreshed the mirror"
    );
    assert_eq!(
        crate::git::rev_parse(&mirror, "refs/fletch/head")
            .await
            .unwrap(),
        head
    );

    // A routed read on the stopped box boots it again.
    let t5 = Instant::now();
    let again = crate::git::rev_parse(&dest, "HEAD").await.unwrap();
    let rebooted = t5.elapsed();
    eprintln!("second boot (lazy, via a routed git read) in {rebooted:.2?}");
    assert_eq!(again, head);
    assert!(matches!(
        registry.state(&agent_id),
        super::BoxPhase::Running { .. }
    ));

    registry.discard(&agent_id).await.unwrap();
    assert!(!box_dir.exists());
    assert!(matches!(
        crate::exec::for_path(&dest),
        crate::exec::Executor::Local
    ));
    let _ = std::fs::remove_dir_all(&identity.writable_root);
    let _ = std::fs::remove_dir_all(&identity.rpc_dir);
}

/// `claude --version` on the tools disk, through the channel with the env
/// file a launch would get.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "boots a real box with a tools disk; see the module docs"]
async fn claude_runs_from_the_tools_disk() {
    let _data = setup().await;
    let agent_id = format!("e2e-cli-{}", std::process::id() % 10_000);
    let identity = identity(&agent_id);
    std::fs::create_dir_all(&identity.writable_root).unwrap();
    let running = BoxRegistry::global()
        .ensure_running(&identity)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let env = super::render_env_file(&[], "claude", &identity.home, super::ENV_DENY).unwrap();
    let env_file = Path::new(fletch_box::guest::ENV_FILE_DIR).join("e2e.env");
    super::executor::put(&running.channel, env.as_bytes(), &env_file, 0o600).unwrap();
    let out = running
        .channel
        .exec(
            Some(&identity.writable_root),
            Some(&env_file),
            &[&fletch_box::tools::bin_path("claude").unwrap(), "--version"],
        )
        .await
        .unwrap();
    eprintln!(
        "claude --version: {} {}",
        String::from_utf8_lossy(&out.stdout).trim(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    assert!(out.status.success());
    BoxRegistry::global().discard(&agent_id).await.unwrap();
}

async fn wait_for(mut done: impl FnMut() -> bool, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A project pinned to boxes must not make providers boxes cannot run
/// unsendable: they are stamped seatbelt. A provider boxes run — one with a
/// guest-home adapter and a tools-disk script — lands in a box once one can
/// boot.
#[cfg(target_os = "macos")]
#[test]
fn new_agents_of_unsupported_providers_fall_back_to_seatbelt() {
    use crate::sandbox::placement::Placement;
    use crate::sandbox::EngineKind;
    for supported in ["claude", "codex", "cursor", "opencode", "pi"] {
        assert!(super::supports_provider(supported), "{supported}");
        assert_eq!(
            super::engine_for_new_agent_with(supported, Placement::Auto, || true),
            EngineKind::Box
        );
    }
    assert!(!super::supports_provider("antigravity"));
    assert_eq!(
        super::engine_for_new_agent_with("antigravity", Placement::Box, || false),
        EngineKind::SandboxExec
    );
}

/// Off macOS there is no seatbelt: every new agent is a box, whatever the
/// placement and whether one can boot right now.
#[cfg(not(target_os = "macos"))]
#[test]
fn off_macos_every_new_agent_is_a_box() {
    use crate::sandbox::placement::Placement;
    use crate::sandbox::EngineKind;
    for placement in [Placement::Auto, Placement::Host, Placement::Box] {
        for ready in [true, false] {
            assert_eq!(
                super::engine_for_new_agent_with("claude", placement, || ready),
                EngineKind::Box
            );
        }
    }
}

/// A box launch's env file points every toolchain into the project's cache
/// root, the same path seatbelt grants that project, with the Linux key on
/// the per-platform rows; nothing in the env file's filtering drops them.
#[test]
fn a_box_launch_redirects_toolchain_caches_into_the_projects_root() {
    let home = Path::new("/Users/u");
    let project = "0b9f3c1e-6a4d-4e8b-9c2f-1d7e5a3b8c90";
    let (root, env) = super::guest_toolchain_cache(home, project).unwrap();
    assert_eq!(
        root,
        crate::sandbox::toolchain_cache_root(home, project).unwrap()
    );
    let file = super::render_env_file(&env, "claude", home, super::ENV_DENY).unwrap();
    let root = root.to_string_lossy();
    let linux = crate::sandbox::BoxPlatform::linux().key();
    for line in [
        format!("BUN_INSTALL_CACHE_DIR={root}/bun"),
        format!("PIP_CACHE_DIR={root}/pip"),
        format!("npm_config_cache={root}/npm/{linux}"),
        format!("PLAYWRIGHT_BROWSERS_PATH={root}/playwright/{linux}"),
    ] {
        assert!(
            file.lines().any(|l| l == line),
            "{line} missing from:\n{file}"
        );
    }
    assert_eq!(
        file.lines().filter(|l| l.contains(&*root)).count(),
        env.len(),
        "every cache variable lands in the env file"
    );
    // A bad project id fails the launch rather than widening the root.
    assert!(super::guest_toolchain_cache(home, "..").is_err());
}

/// An image directory counts as installed only for this build's version,
/// only once it carries the manifest a verified download leaves behind, and
/// only with every enabled layer in place.
#[test]
fn only_a_verified_image_of_this_builds_version_is_installed() {
    let _serial = settings::TEST_LOCK.blocking_lock();
    let data = tempfile::tempdir().unwrap();
    crate::sandbox::set_data_dir(data.path().to_path_buf());
    settings::set(BoxSettings::default());
    let pin = super::pinned_image();
    let images = super::data_root().join("images");
    let manifest = |version: &str| {
        serde_json::json!({
            "version": version, "arch": std::env::consts::ARCH,
            "images": [
                {"name": "base", "file": "base.erofs", "size": 5, "sha256": "0".repeat(64)},
                {"name": "devtools", "file": "layer-devtools.erofs", "size": 5, "sha256": "1".repeat(64)},
                {"name": "browser", "file": "layer-browser.erofs", "size": 5, "sha256": "2".repeat(64)},
            ],
        })
        .to_string()
    };
    let other = images.join("99.0.0");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("base.erofs"), b"a newer image").unwrap();
    std::fs::write(other.join("manifest.json"), manifest("99.0.0")).unwrap();
    let dir = images.join(&pin.version);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("base.erofs"), b"not really an image").unwrap();
    assert!(
        super::installed_image().unwrap().is_none(),
        "no manifest yet"
    );
    assert_eq!(super::setup::image_status().phase, "not_installed");

    std::fs::write(dir.join("manifest.json"), manifest(&pin.version)).unwrap();
    assert!(
        super::installed_image().unwrap().is_none(),
        "the layers are not there yet"
    );
    std::fs::write(dir.join("layer-devtools.erofs"), b"layer").unwrap();
    assert!(
        super::installed_image().unwrap().is_none(),
        "a layer still on its way holds the image back"
    );
    std::fs::write(dir.join("layer-browser.erofs"), b"layer").unwrap();
    let installed = super::installed_image().unwrap().expect("the pinned image");
    assert_eq!(installed.version, pin.version);
    assert_eq!(installed.layers.len(), 2);
    assert_eq!(super::setup::image_status().phase, "installed");
}

thread_local! {
    /// The `box:*` events this thread emitted, while a test is listening.
    static EVENTS: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

/// Record an emitted event, for the test on this thread that asked to.
pub(super) fn note_event(event: &str) {
    EVENTS.with(|events| {
        if let Some(events) = events.borrow_mut().as_mut() {
            events.push(event.to_string());
        }
    });
}

/// The registry says when the set of VMs changes, which is what keeps a client's
/// list honest about a hidden workspace: nothing else tells it one is gone. A
/// box it already knows is not news.
#[tokio::test(flavor = "current_thread")]
async fn the_list_event_fires_when_a_box_is_registered_or_discarded() {
    let id = "list-event-test";
    let identity = BoxIdentity::new(id, "p", "claude").unwrap();
    let registry = BoxRegistry::global();
    let list_events = || {
        EVENTS.with(|events| {
            let events = events.borrow();
            let events = events.as_ref().unwrap();
            events.iter().filter(|e| *e == super::EVENT_LIST).count()
        })
    };
    EVENTS.with(|events| *events.borrow_mut() = Some(Vec::new()));

    registry.register(identity.clone());
    assert_eq!(list_events(), 1, "a new box");
    registry.register(identity);
    assert_eq!(list_events(), 1, "a box it knows");

    super::discard_if_any(id).await.unwrap();
    assert_eq!(list_events(), 2, "a discarded box");
    super::discard_if_any(id).await.unwrap();
    assert_eq!(list_events(), 2, "nothing left to discard");

    EVENTS.with(|events| *events.borrow_mut() = None);
}

/// A box exists once it has a data disk: a tools build's throwaway box and a
/// directory that never got one are not VMs.
#[test]
fn a_created_box_is_a_directory_with_a_data_disk() {
    let root = tempfile::tempdir().unwrap();
    let disk = |dir: &str, with_disk: bool| {
        let dir = root.path().join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        if with_disk {
            std::fs::write(dir.join(super::registry::DATA_DISK), b"").unwrap();
        }
    };
    disk("vm", true);
    disk("never-booted", false);
    disk("tools-build", true);

    let boxes = super::boxes_in(root.path());
    assert_eq!(
        boxes.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["vm"]
    );
}

// `block_on` from the sync seams (`launch_agent`, the executor).

/// More sync waits than the runtime has workers, each on a worker and
/// waiting on a task spawned onto that same runtime (and its timer), all
/// finish. Bounded, so a regression fails instead of hanging the suite.
#[test]
fn sync_waits_on_every_worker_still_let_the_runtime_run() {
    const WAITS: usize = 6;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    for i in 0..WAITS {
        let handle = runtime.handle().clone();
        let done_tx = done_tx.clone();
        runtime.spawn(async move {
            // A sync seam called from async code, as `launch_agent` is.
            let value = block_on_with(Some(handle), async move {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    i
                })
                .await
                .unwrap()
            });
            let _ = done_tx.send(value);
        });
    }
    let mut done: Vec<usize> = (0..WAITS)
        .map_while(|_| done_rx.recv_timeout(Duration::from_secs(10)).ok())
        .collect();
    // A starved runtime never finishes its shutdown; leave it behind.
    runtime.shutdown_background();
    done.sort_unstable();
    assert_eq!(done, (0..WAITS).collect::<Vec<_>>(), "the runtime starved");
}

/// A current-thread runtime cannot `block_in_place`; the wait still runs, on
/// that runtime's handle or a throwaway one.
#[tokio::test(flavor = "current_thread")]
async fn a_current_thread_caller_does_not_panic() {
    let handle = tokio::runtime::Handle::current();
    assert_eq!(block_on_with(Some(handle), async { 7 }), 7);
    assert_eq!(block_on_with(None, async { 8 }), 8);
}

// The reviewer's confined view (`confined_view`, `enforced_view`).

const HOME: &str = "/Users/u";
const CODER_ROOT: &str = "/Users/u/.fletch/workspaces/coder";
const CODER_RPC: &str = "/Users/u/.fletch/rpc/coder";
const OWN_ROOT: &str = "/Users/u/.fletch/workspaces/reviewer";
const OWN_RPC: &str = "/Users/u/.fletch/rpc/reviewer";
const CACHE: &str = "/Users/u/.fletch/toolchain-cache/p1";

fn launch_ctx<'a>(
    writable_root: &'a Path,
    cwd: &'a Path,
    adopted_read_only: bool,
) -> AgentLaunchCtx<'a> {
    AgentLaunchCtx {
        agent_id: "reviewer",
        project_id: "p1",
        provider: "claude",
        writable_root,
        source_repos: &[],
        rpc_dir: Path::new(OWN_RPC),
        cwd,
        home: Path::new(HOME),
        interactive: true,
        blackboard: None,
        account_dir: None,
        oauth_token: None,
        codex_home: None,
        adopted_read_only,
        folder_grants: &[],
    }
}

fn grant(path: &str, writable: bool) -> FolderGrant {
    FolderGrant {
        path: PathBuf::from(path),
        writable,
    }
}

/// The coder's box: its root, mailbox and home as the launch context derives
/// them, with `shares`.
fn coder_box(writable_root: &str, shares: Vec<FolderGrant>) -> BoxIdentity {
    BoxIdentity {
        agent_id: "coder".into(),
        project_id: "p1".into(),
        provider: "claude".into(),
        writable_root: PathBuf::from(writable_root),
        rpc_dir: PathBuf::from(CODER_RPC),
        home: PathBuf::from(HOME),
        shares,
    }
}

/// Under the reviewer's own root, the store or copy named `name`.
fn own_copy(name: &str) -> PathBuf {
    Path::new(OWN_ROOT).join(super::COW_DIR).join(name)
}

/// The local reviewer, a tenant adopting the coder's checkout to read it,
/// writes nothing it shares with the coder: everything the guest exposes of
/// the coder's is read-only — the home, which holds the coder's root and
/// mailbox, and the folders the box shares writable that the reviewer was
/// not granted writable — its own paths are writable again, and every shared
/// location a CLI writes is a private copy under its own root.
#[test]
fn a_reviewer_tenant_writes_only_its_own_paths() {
    let tree = Path::new(CODER_ROOT).join("repo");
    let shares = vec![
        grant("/Volumes/data", true),
        grant("/Volumes/ref", false),
        grant("/Volumes/both", true),
        grant("/Users/u/notes", true),
    ];
    let host = coder_box(CODER_ROOT, shares);
    // Its own grants: two of the box's writable shares writable, one
    // read-only.
    let grants = [
        grant("/Volumes/data", false),
        grant("/Volumes/both", true),
        grant("/Users/u/notes", true),
    ];
    let ctx = AgentLaunchCtx {
        folder_grants: &grants,
        ..launch_ctx(Path::new(OWN_ROOT), &tree, true)
    };
    let view = super::confined_view(&ctx, true, &host, Path::new(CACHE)).unwrap();

    // The coder's root and mailbox lie in the home, whose read-only tree
    // takes them along; a read-only share needs nothing, and one the
    // reviewer may write keeps the guest's mode.
    let read_only: Vec<PathBuf> = [HOME, "/Volumes/data"].map(PathBuf::from).into();
    assert_eq!(view.read_only, read_only);

    // Writable again: its own root and mailbox, and the share it was granted
    // inside the home (the other one lies in no read-only tree).
    let mut expected: Vec<PathBuf> = [OWN_ROOT, OWN_RPC, "/Users/u/notes"]
        .map(PathBuf::from)
        .into();
    expected.sort();
    let mut writable = view.writable_dirs.clone();
    writable.sort();
    assert_eq!(writable, expected);

    // A private copy each: the toolchain cache, the provider's state dirs and
    // the scratch dirs, with their stores under its own root, and
    // `~/.claude.json`, with its copy there.
    let home = Path::new(HOME);
    let mut shared: Vec<PathBuf> = vec![PathBuf::from(CACHE)];
    shared.extend(policy::provider_state_dirs("claude", home));
    shared.extend(policy::agent_scratch_dirs(home));
    let cow: Vec<PathBuf> = view.cow_dirs.iter().map(|(dir, _)| dir.clone()).collect();
    assert_eq!(cow, shared);
    let store_of = |dir: &Path| -> Option<PathBuf> {
        let (_, store) = view.cow_dirs.iter().find(|(d, _)| d == dir)?;
        Some(store.clone())
    };
    let cache_store = own_copy(".fletch%2Ftoolchain-cache%2Fp1");
    assert_eq!(store_of(Path::new(CACHE)), Some(cache_store));
    let support = home.join("Library/Application Support");
    let support_store = own_copy("Library%2FApplication Support");
    assert_eq!(store_of(&support), Some(support_store));
    assert_eq!(
        view.cow_files,
        [(home.join(".claude.json"), own_copy(".claude.json"))]
    );

    // Each inside a read-only tree; nothing writable in anything of the
    // coder's; every store and copy in its own root.
    let held = |path: &Path| view.read_only.iter().any(|dir| super::inside(path, dir));
    for path in view.writable() {
        assert!(held(path), "{}", path.display());
        for coders in [CODER_ROOT, CODER_RPC, "/Volumes/data"] {
            assert!(!path.starts_with(coders), "{}", path.display());
        }
    }
    let stores = Path::new(OWN_ROOT).join(super::COW_DIR);
    for (path, private) in view.cow_dirs.iter().chain(&view.cow_files) {
        assert!(held(path), "{}", path.display());
        assert!(super::inside(private, &stores), "{}", private.display());
    }
    // Parents first: no path is bound before a directory that holds it.
    let dirs = &view.writable_dirs;
    for (i, dir) in dirs.iter().enumerate() {
        assert!(
            !dirs[i + 1..].iter().any(|later| dir.starts_with(later)),
            "{} comes before its parent",
            dir.display()
        );
    }
}

/// A Cursor reviewer cannot plant an MCP config the coder's next launch
/// runs: `~/.cursor` is not writable, it is a copy-on-write view, and what
/// the reviewer writes at `~/.cursor/mcp.json`, a file that need not exist
/// yet, lands in its own store.
#[test]
fn a_reviewers_writes_to_shared_state_land_in_its_own_copy() {
    let tree = Path::new(CODER_ROOT).join("repo");
    let host = coder_box(CODER_ROOT, Vec::new());
    let ctx = AgentLaunchCtx {
        provider: "cursor",
        ..launch_ctx(Path::new(OWN_ROOT), &tree, true)
    };
    let view = super::confined_view(&ctx, true, &host, Path::new(CACHE)).unwrap();
    let cursor = Path::new(HOME).join(".cursor");
    let writes = |path: &Path| view.writable_dirs.iter().any(|dir| path.starts_with(dir));
    assert!(!writes(&cursor.join("mcp.json")));
    let pair = (cursor.clone(), own_copy(".cursor"));
    assert!(view.cow_dirs.contains(&pair), "{:?}", view.cow_dirs);
    assert_eq!(
        view.private_path(&cursor.join("mcp.json")),
        Some(own_copy(".cursor/upper/mcp.json"))
    );
    assert!(view.cow_files.is_empty());
}

/// A folder grant is matched against the box's shares by containment: a
/// grant inside a share is carved out of it writable, the share staying
/// read-only; a share inside a grant is the reviewer's whole; a grant no
/// share covers is not in the guest, and a share no grant covers is
/// read-only, with the shares nested in it. A shared directory that holds a
/// mount stays read-only rather than hide it under an overlay.
#[test]
fn nested_grants_and_shares_are_matched_by_containment() {
    let tree = Path::new(CODER_ROOT).join("repo");
    let shares = vec![
        grant("/Volumes/project", true),
        grant("/Volumes/project/sub", true),
        grant("/Volumes/work/docs", true),
        grant("/Users/u/notes", true),
        grant("/Users/u/.cache/models", false),
    ];
    let host = coder_box(CODER_ROOT, shares);
    let grants = [
        grant("/Volumes/project/docs", true),
        grant("/Volumes/project/sub", false),
        grant("/Volumes/work", true),
        grant("/Volumes/elsewhere", true),
        grant("/Users/u", true),
    ];
    let ctx = AgentLaunchCtx {
        folder_grants: &grants,
        ..launch_ctx(Path::new(OWN_ROOT), &tree, true)
    };
    let view = super::confined_view(&ctx, true, &host, Path::new(CACHE)).unwrap();

    // The project share is the coder's, its nested share with it; the work
    // docs lie in the reviewer's grant and stay out; a grant of the whole
    // home does not make the coder's home binds the reviewer's.
    let read_only: Vec<PathBuf> = [HOME, "/Volumes/project"].map(PathBuf::from).into();
    assert_eq!(view.read_only, read_only);
    let writable = |path: &str| view.writable_dirs.contains(&PathBuf::from(path));
    // Carved out of the read-only project share; the notes share, inside
    // the home grant, carved out of the read-only home.
    assert!(
        writable("/Volumes/project/docs"),
        "{:?}",
        view.writable_dirs
    );
    assert!(writable("/Users/u/notes"), "{:?}", view.writable_dirs);
    // Outside every read-only tree (the guest's own mode), not mounted at
    // all, granted read-only, or the coder's home.
    for path in [
        "/Volumes/work/docs",
        "/Volumes/work",
        "/Volumes/elsewhere",
        "/Volumes/project/sub",
        HOME,
    ] {
        assert!(!writable(path), "{path}");
    }

    // `~/.cache` holds a share, which an overlay would hide: read-only; the
    // other scratch dirs are copies as ever.
    let cow = |path: &Path| view.cow_dirs.iter().any(|(dir, _)| dir == path);
    let home = Path::new(HOME);
    assert!(!cow(&home.join(".cache")), "{:?}", view.cow_dirs);
    assert!(cow(&home.join(".npm")), "{:?}", view.cow_dirs);
}

/// The three identity binds all go in when none holds the others, and a
/// codex reviewer's `CODEX_HOME` is writable, with no claude state file.
#[test]
fn a_root_outside_the_home_is_read_only_too() {
    let tree = Path::new("/w/coder/repo");
    let host = coder_box("/w/coder", Vec::new());
    let codex_home = Path::new(OWN_ROOT).join("codex-home");
    let ctx = AgentLaunchCtx {
        provider: "codex",
        codex_home: Some(codex_home.as_path()),
        ..launch_ctx(Path::new(OWN_ROOT), tree, true)
    };
    let view = super::confined_view(&ctx, true, &host, Path::new(CACHE)).unwrap();
    let read_only: Vec<PathBuf> = ["/w/coder", HOME].map(PathBuf::from).into();
    assert_eq!(view.read_only, read_only);
    let at = |path: &Path| view.writable_dirs.iter().position(|dir| dir == path);
    let root = at(Path::new(OWN_ROOT)).unwrap();
    let overlay = at(&codex_home).unwrap();
    assert!(root < overlay, "{:?}", view.writable_dirs);
    assert!(view.cow_files.is_empty());
}

/// An owner is never confined, nor is a tenant that adopts its tree to
/// write it (a workflow step); the guest is not even asked.
#[test]
fn owners_and_workflow_steps_are_not_confined() {
    let root = Path::new(CODER_ROOT);
    let own_tree = root.join("repo");
    let run_tree = Path::new("/runs/run-7/repo");
    let host = coder_box(CODER_ROOT, Vec::new());
    let cases = [
        // An owner, in its own checkout or in an adopted one.
        (launch_ctx(root, &own_tree, true), false),
        (launch_ctx(root, run_tree, true), false),
        // A tenant that adopts its tree to write it.
        (launch_ctx(Path::new(OWN_ROOT), run_tree, false), true),
    ];
    for (ctx, is_tenant) in &cases {
        let view = super::confined_view(ctx, *is_tenant, &host, Path::new(CACHE));
        assert_eq!(view, None, "{}", ctx.cwd.display());
        let enforced = super::enforced_view(view, |_| panic!("the guest was asked"));
        assert_eq!(enforced.unwrap(), None);
    }
}

/// A view the guest enforces is the launch's; a guest that predates it, or
/// cannot set it up, refuses the launch: there is no unconfined fallback.
#[test]
fn a_guest_that_cannot_confine_refuses_the_reviewer() {
    let view = super::ConfinedView {
        read_only: vec![PathBuf::from(HOME)],
        ..Default::default()
    };
    let enforced = super::enforced_view(Some(view.clone()), |_| Ok(true));
    assert_eq!(enforced.unwrap().as_ref(), Some(&view));

    match super::enforced_view(Some(view.clone()), |_| Ok(false)) {
        Err(Error::SandboxUnavailable(why)) => {
            assert!(why.contains("update the box image"), "{why}");
        }
        other => panic!("an old guest must refuse the launch: {other:?}"),
    }
    let cannot = |_: &super::ConfinedView| -> crate::error::Result<bool> {
        Err(Error::SandboxUnavailable("exit status: 125".into()))
    };
    assert!(super::enforced_view(Some(view), cannot).is_err());
}

/// The launch, its probe and the step that makes the directories it mounts
/// all carry the same view.
#[test]
fn a_confined_launch_renders_the_view_it_was_given() {
    let home = Path::new(HOME);
    let support = home.join("Library/Application Support");
    let view = super::ConfinedView {
        read_only: vec![home.to_path_buf(), PathBuf::from("/Volumes/my data")],
        writable_dirs: vec![
            PathBuf::from(OWN_ROOT),
            PathBuf::from("/Volumes/my data/own"),
        ],
        cow_dirs: vec![(support, own_copy("Library%2FApplication Support"))],
        cow_files: vec![(home.join(".claude.json"), own_copy(".claude.json"))],
    };
    let tree = Path::new(CODER_ROOT).join("repo");
    let env_file = Path::new("/data/run/1.env");
    let remote = super::agent_remote_command(&tree, env_file, Some(&view), "claude");
    let stores = "/Users/u/.fletch/workspaces/reviewer/.fletch-cow";
    assert_eq!(
        remote.head,
        ["/opt/fletch/bin/fletch-guest", "exec", "--args-file"]
    );
    assert_eq!(
        remote.tail,
        [
            "--env-file",
            "/data/run/1.env",
            "--read-only",
            "/Users/u",
            "--read-only",
            "'/Volumes/my data'",
            "--writable",
            "/Users/u/.fletch/workspaces/reviewer",
            "--writable",
            "'/Volumes/my data/own'",
            "--cow",
            "'/Users/u/Library/Application Support'",
            &format!("'{stores}/Library%2FApplication Support'"),
            "--cow-file",
            "/Users/u/.claude.json",
            &format!("{stores}/.claude.json"),
            "--cwd",
            "/Users/u/.fletch/workspaces/coder/repo",
            "--",
            "claude"
        ]
    );
    let plain = super::agent_remote_command(&tree, env_file, None, "claude");
    let options = ["--read-only", "--writable", "--cow", "--cow-file"].map(String::from);
    assert!(!plain.tail.iter().any(|w| options.contains(w)), "{plain:?}");
    assert_eq!(plain.head, remote.head);

    assert_eq!(
        super::probe_words(&view),
        [
            "/opt/fletch/bin/fletch-guest",
            "exec",
            "--read-only",
            "/Users/u",
            "--read-only",
            "/Volumes/my data",
            "--writable",
            "/Users/u/.fletch/workspaces/reviewer",
            "--writable",
            "/Volumes/my data/own",
            "--cow",
            "/Users/u/Library/Application Support",
            &format!("{stores}/Library%2FApplication Support"),
            "--cow-file",
            "/Users/u/.claude.json",
            &format!("{stores}/.claude.json"),
            "--env-file",
            "/dev/null",
            "--",
            "/bin/true"
        ]
    );
    // The directories it binds and overlays, never the stores or copies,
    // which the guest makes inside the confinement.
    assert_eq!(
        super::carve_out_script(&view),
        "umask 077 && mkdir -p -- /Users/u/.fletch/workspaces/reviewer '/Volumes/my data/own' \
         '/Users/u/Library/Application Support'"
    );
}

/// An agent's launcher is ssh running the launch's command, which names the
/// process's args file: over the box's shared connection, with a tty only for
/// the PTY view. A command too long for that connection (a view of very many
/// paths) takes one of its own.
#[test]
fn an_agent_launch_shares_the_box_connection_unless_its_command_is_long() {
    let launch_id = "8a0c4f2e-1d3b-4c5a-9e7f-0123456789ab";
    let config = Path::new("/b/ssh_config");
    let args_file = super::launch_files::stem(launch_id).with_extension("7.args");
    assert_eq!(
        args_file,
        Path::new(&format!("/data/run/{launch_id}.7.args"))
    );
    let tree = Path::new(CODER_ROOT).join("repo");
    let env_file = super::launch_files::env_file(launch_id);
    let claude = "/opt/fletch/tools/claude/bin/claude";
    let command = super::agent_remote_command(&tree, &env_file, None, claude);
    let remote = command.with_args_file(&args_file);
    assert_eq!(
        remote,
        format!(
            "/opt/fletch/bin/fletch-guest exec --args-file {} {}",
            args_file.display(),
            command.tail.join(" ")
        )
    );
    assert!(remote.len() < super::executor::SHARED_CONNECTION_COMMAND_MAX);
    assert_eq!(
        super::agent_launcher(config, false, &args_file, &command),
        ["-F", "/b/ssh_config", "-T", "box", remote.as_str()]
    );
    assert_eq!(
        super::agent_launcher(config, true, &args_file, &command),
        ["-F", "/b/ssh_config", "-tt", "box", remote.as_str()]
    );

    let view = super::ConfinedView {
        read_only: vec![PathBuf::from(HOME)],
        writable_dirs: (0..100)
            .map(|n| PathBuf::from(format!("{OWN_ROOT}/granted/a-folder-with-a-long-name-{n}")))
            .collect(),
        ..Default::default()
    };
    let long = super::agent_remote_command(&tree, &env_file, Some(&view), claude);
    let launcher = super::agent_launcher(config, false, &args_file, &long);
    assert!(launcher.last().unwrap().len() > super::executor::SHARED_CONNECTION_COMMAND_MAX);
    assert_eq!(&launcher[..5], ["-F", "/b/ssh_config", "-S", "none", "-T"]);
}

/// The launch command reaches the guest shell as the words it was built from,
/// the args file's path one of them however it is spelled: a fake guest agent
/// stands in for `fletch-guest` and prints the words it was given.
#[test]
fn an_agent_launchs_command_reaches_the_guest_shell_word_for_word() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let guest = dir.path().join("guest exec");
    std::fs::write(&guest, "#!/bin/sh\nprintf '%s\\0' \"$@\"\n").unwrap();
    std::fs::set_permissions(&guest, std::fs::Permissions::from_mode(0o755)).unwrap();
    let quote = fletch_box::channel::shell_quote;
    let command = super::AgentCommand {
        head: vec![quote(&guest.to_string_lossy())],
        tail: ["--cwd", "/w s/it's", "--", "claude"].map(quote).to_vec(),
    };
    let args_file = Path::new("/data/run/a b's.1.args");
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command.with_args_file(args_file))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let words: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|w| !w.is_empty())
        .map(|w| String::from_utf8_lossy(w).into_owned())
        .collect();
    assert_eq!(
        words,
        [
            "/data/run/a b's.1.args",
            "--cwd",
            "/w s/it's",
            "--",
            "claude"
        ]
    );
}

/// What the host does in the guest for a launch, its seed and its credential
/// files, runs where the launch runs: for a confined launch under
/// `fletch-guest exec` with the launch's view (which the probe enters too),
/// so a removal there is a whiteout in its own copy; for any other launch as
/// it is.
#[test]
fn the_hosts_operations_for_a_launch_run_in_its_view() {
    let home = Path::new(HOME);
    let view = super::ConfinedView {
        read_only: vec![home.to_path_buf()],
        writable_dirs: vec![PathBuf::from(OWN_ROOT)],
        cow_dirs: vec![(home.join(".pi"), own_copy(".pi"))],
        cow_files: Vec::new(),
    };
    let rm = ["rm", "-f", "--", "/Users/u/.pi/agent/auth.json"];
    let mut confined = view.exec_words();
    assert_eq!(
        &confined[..],
        &[
            "/opt/fletch/bin/fletch-guest",
            "exec",
            "--read-only",
            "/Users/u",
            "--writable",
            OWN_ROOT,
            "--cow",
            "/Users/u/.pi",
            "/Users/u/.fletch/workspaces/reviewer/.fletch-cow/.pi",
            "--env-file",
            "/dev/null",
            "--",
        ]
    );
    confined.extend(rm.map(String::from));
    assert_eq!(view.scope().words(&rm), confined);
    assert_eq!(super::executor::Scope::default().words(&rm), rm);

    let mut probe = view.exec_words();
    probe.push("/bin/true".into());
    assert_eq!(super::probe_words(&view), probe);
}

/// Before a confined launch's view is set up, the files it copies are made
/// from its seed in the guest's own view, and only where nothing is yet: the
/// copy is bound on the shared file, which must exist. The rest of the seed
/// is applied in the view.
#[test]
fn only_the_files_a_view_copies_are_seeded_outside_it() {
    use crate::sandbox::provider::GuestEntry;

    let tree = Path::new(CODER_ROOT).join("repo");
    let host = coder_box(CODER_ROOT, Vec::new());
    let ctx = launch_ctx(Path::new(OWN_ROOT), &tree, true);
    let view = super::confined_view(&ctx, true, &host, Path::new(CACHE)).unwrap();
    let home = Path::new(HOME);
    let file = |path: PathBuf, keep_existing| GuestEntry::File {
        path,
        bytes: b"{}".to_vec(),
        mode: 0o600,
        keep_existing,
    };
    let seed = [
        GuestEntry::Dir(home.join(".claude/projects")),
        file(home.join(".claude.json"), false),
        file(home.join(".claude/projects/x.json"), false),
        GuestEntry::Absent(home.join(".claude.json")),
    ];
    assert_eq!(
        super::mount_point_seed(&view, &seed),
        [file(home.join(".claude.json"), true)]
    );
}

/// What the host writes for a confined Pi or OpenCode launch, its login files
/// and its seeded config, lies in the launch's private copies at the guest's
/// own paths, so in its view they are its own and the coder's are untouched.
#[test]
fn a_reviewers_login_and_seed_land_in_its_own_copies() {
    let tree = Path::new(CODER_ROOT).join("repo");
    let host = coder_box(CODER_ROOT, Vec::new());
    let home = Path::new(HOME);
    let pi = [".pi/agent/auth.json", ".pi/agent/settings.json"];
    let opencode = [
        ".local/share/opencode/auth.json",
        ".local/share/opencode/opencode.db",
        ".config/opencode/opencode.json",
    ];
    for (provider, paths) in [("pi", &pi[..]), ("opencode", &opencode[..])] {
        let ctx = AgentLaunchCtx {
            provider,
            ..launch_ctx(Path::new(OWN_ROOT), &tree, true)
        };
        let view = super::confined_view(&ctx, true, &host, Path::new(CACHE)).unwrap();
        for path in paths {
            let path = home.join(path);
            assert!(
                view.private_path(&path).is_some(),
                "{provider}: {}",
                path.display()
            );
        }
    }
}

/// A confined launch's own files are found where it wrote them: under a
/// copy-on-write directory in its store's upper layer, a copy-on-write file
/// in its copy; the host finds the same paths without the launch's view.
#[test]
fn a_reviewers_sessions_are_read_from_its_private_copy() {
    let tree = Path::new(CODER_ROOT).join("repo");
    let host = coder_box(CODER_ROOT, Vec::new());
    let ctx = launch_ctx(Path::new(OWN_ROOT), &tree, true);
    let view = super::confined_view(&ctx, true, &host, Path::new(CACHE)).unwrap();
    let home = Path::new(HOME);
    let projects = home.join(".claude/projects");
    let upper = own_copy(".claude%2Fprojects/upper");
    let transcript = Path::new("-Users-u-repo/1.jsonl");
    let claude_json = home.join(".claude.json");
    for (shared, private) in [
        (projects.clone(), upper.clone()),
        (projects.join(transcript), upper.join(transcript)),
        (claude_json, own_copy(".claude.json")),
    ] {
        assert_eq!(view.private_path(&shared), Some(private.clone()));
        let found = super::tenant_private_path("claude", home, Path::new(OWN_ROOT), &shared);
        assert_eq!(found, Some(private), "{}", shared.display());
    }
    // Its own paths, and what is read-only, stay where they are.
    for path in [
        Path::new(OWN_ROOT).join("x"),
        home.join(".claude/settings.json"),
    ] {
        assert_eq!(view.private_path(&path), None, "{}", path.display());
    }
}

/// One flat name per shared path under the reviewer's root, readable, and
/// never the same for two paths.
#[test]
fn copy_on_write_names_are_flat_and_distinct() {
    let home = Path::new(HOME);
    let name = |path: &str| super::cow_name(home, Path::new(path));
    assert_eq!(name("/Users/u/.cache"), ".cache");
    assert_eq!(name("/Users/u/.local/share"), ".local%2Fshare");
    assert_eq!(name("/Users/u/a%2Fb"), "a%252Fb");
    assert_eq!(name("/opt/cache"), "%2Fopt%2Fcache");
    assert_ne!(name("/Users/u/a/b"), name("/Users/u/a%2Fb"));
    assert_ne!(name("/Users/u/opt/cache"), name("/opt/cache"));
}

/// The probe tells a guest that predates the options (it names one unknown)
/// from one that has them and could not set them up.
#[test]
fn the_read_only_probe_tells_an_old_guest_from_a_failed_one() {
    assert_eq!(super::read_only_probe_answer(true, ""), Some(true));
    for option in ["--read-only", "--writable", "--cow", "--cow-file"] {
        let old = format!("fletch-guest: exec: unknown option {option}\n");
        assert_eq!(super::read_only_probe_answer(false, &old), Some(false));
    }
    let failed = "fletch-guest: exec: unshare: Operation not permitted (os error 1)\n";
    assert_eq!(super::read_only_probe_answer(false, failed), None);
    assert_eq!(super::read_only_probe_answer(false, ""), None);
}
