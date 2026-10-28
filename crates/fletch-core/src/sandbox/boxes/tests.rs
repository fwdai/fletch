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
//! a temporary data dir. A tools disk is built when `FLETCH_BOX_TOOLS_DISK`
//! names an existing one to attach instead (a build takes minutes).
//!
//! The runtime is multi-threaded on purpose: the sync seams boot a box from a
//! scoped thread with `Handle::block_on`, which a current-thread runtime
//! cannot drive.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::provision::Landing;
use super::registry::{BoxIdentity, BoxRegistry, StopReason};
use super::settings::{self, BoxSettings};
use crate::sandbox::provision::CheckoutSpec;

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} must be set")))
}

/// Points the engine at a temp data dir, installs the image from
/// `FLETCH_BOX_IMAGE_DIR`, and turns the flag on. Returns the data dir guard.
async fn setup() -> tempfile::TempDir {
    let data = tempfile::tempdir().unwrap();
    crate::sandbox::set_data_dir(data.path().to_path_buf());
    let image_dir = env_path("FLETCH_BOX_IMAGE_DIR");
    settings::set(BoxSettings {
        enabled: true,
        image_url: Some(format!("file://{}", image_dir.display())),
        idle_stop: Some(Duration::from_secs(600)),
        ..BoxSettings::default()
    });
    let t0 = Instant::now();
    let installed = super::image_store()
        .install(&super::pinned_image(), |_| {
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
        let version = super::registry::tools_version("claude");
        let target = fletch_box::tools::disk_path(&super::tools_root(), "claude", &version);
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
         tools disk attached: {})",
        running.tools_disk.is_some()
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

/// Enabling the pilot must not make providers boxes cannot run unsendable:
/// they are stamped with the default engine, and nothing else is touched.
#[test]
fn new_agents_of_unsupported_providers_fall_back_to_seatbelt() {
    use crate::sandbox::EngineKind;
    assert_eq!(
        super::engine_for_new_agent(EngineKind::Box, "claude"),
        EngineKind::Box
    );
    assert_eq!(
        super::engine_for_new_agent(EngineKind::Box, "codex"),
        EngineKind::SandboxExec
    );
    assert_eq!(
        super::engine_for_new_agent(EngineKind::Docker, "codex"),
        EngineKind::Docker
    );
}

/// An image directory of the pinned version counts as installed only when its
/// recorded hash is this build's pin: a same-version image from another build
/// reads as absent, so the install path replaces it instead of a spawn failing
/// at the per-boot check.
#[test]
fn a_same_version_image_with_another_hash_is_not_installed() {
    let _serial = settings::TEST_LOCK.blocking_lock();
    let data = tempfile::tempdir().unwrap();
    crate::sandbox::set_data_dir(data.path().to_path_buf());
    settings::set(BoxSettings::default());
    let pin = super::pinned_image();
    let dir = super::data_root().join("images").join(&pin.version);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("base.erofs"), b"not really an image").unwrap();

    std::fs::write(
        dir.join("base.erofs.sha256"),
        format!("{}\n", "0".repeat(64)),
    )
    .unwrap();
    assert!(super::installed_image().unwrap().is_none());
    assert_eq!(super::setup::image_status().phase, "not_installed");

    std::fs::write(dir.join("base.erofs.sha256"), format!("{}\n", pin.sha256)).unwrap();
    let installed = super::installed_image().unwrap().expect("the pinned image");
    assert_eq!(installed.version, pin.version);
    assert_eq!(super::setup::image_status().phase, "installed");
}
