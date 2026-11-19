//! The runner against a provider account in memory (`cloud::fake`): what is
//! made and where, what each state of its machine makes `status`, `start` and
//! `stop` do, that a failed create leaves nothing behind, and the order and
//! refusals of a delete. No ssh: the set-up over the channel is a stand-in
//! here, and its text (the start script, the env file, the release's address
//! and digest) is tested on its own.

use std::collections::HashMap;

use parking_lot::Mutex;

use super::setup::{self, ENV_FILE, HOST_BIN};
use super::*;
use crate::cloud::fake::{FakeCloud, Shape};
use crate::sandbox::boxes::CloudBoxNotDestroyed;

fn record() -> RunnerRecord {
    RunnerRecord {
        host_id: "aG9zdC1rZXktMDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3".into(),
        name: "Fletch runner".into(),
        relay: "wss://relay.example".into(),
    }
}

/// What `create` told its caller, in order.
#[derive(Default)]
struct Heard(Mutex<Vec<Progress>>);

impl Heard {
    fn hear(&self) -> impl Fn(Progress) + Sync + '_ {
        move |step| self.0.lock().push(step)
    }
}

/// A runner made in `dir` at `fake`, its set-up standing in.
async fn made(fake: &FakeCloud, dir: &Path) -> RunnerRecord {
    create_in(
        dir,
        |_| Ok(fake.client()),
        ProviderId::ALL[0],
        &|_| {},
        |_| async { Ok(record()) },
    )
    .await
    .unwrap()
}

/// The runner's directory under a temp dir, not made yet.
fn runner_dir(root: &tempfile::TempDir) -> PathBuf {
    root.path().join("cloud-runner")
}

/// The machine satisfies the entrypoint's contract with paths that put the
/// service's home, and its data directory, on the disk and outside `/data`,
/// where a box's guest refuses an identity path: the runner's paths are its
/// workspaces' identity paths. Kept running by its provider, a box's size,
/// the pinned image, and nothing secret.
#[test]
fn the_machine_meets_the_entrypoints_contract_with_its_home_on_the_disk() {
    let spec = spec("ssh-ed25519 AAAA runner".into());
    let env: HashMap<&str, &str> = spec
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        env,
        HashMap::from([
            ("FLETCH_BOX_HOME", "/home/agent"),
            ("FLETCH_BOX_RPC_DIR", "/home/agent/.fletch-runner/rpc"),
            ("FLETCH_BOX_WRITABLE_ROOT", "/home/agent/.fletch-runner/ws"),
            ("FLETCH_BOX_AUTHORIZED_KEY", "ssh-ed25519 AAAA runner"),
            ("FLETCH_BOX_ID", "runner"),
            ("FLETCH_BOX_SSH_PORT", "2222"),
        ])
    );
    for path in [HOME, DATA_DIR, UNUSED_RPC_DIR, UNUSED_WRITABLE_ROOT] {
        assert!(path.starts_with("/home/"), "{path}: /home is in the image");
        assert!(!path.starts_with("/data"), "{path}");
    }
    assert!(DATA_DIR.starts_with(&format!("{HOME}/")), "on the disk");
    for path in [UNUSED_RPC_DIR, UNUSED_WRITABLE_ROOT] {
        assert!(
            path.starts_with(&format!("{HOME}/")),
            "bound inside the home, which is bound first: {path}"
        );
    }
    assert!(spec.keep_running);
    assert_eq!(spec.name, "runner");
    assert_eq!(spec.image, boxes_cloud::image());
    assert_eq!(spec.ssh_port, guest::CLOUD_SSH_PORT);
    assert_eq!(
        (spec.cpus, spec.memory_mib, spec.disk_gib),
        (2, 2048, Some(settings::DEFAULT_DISK_GIB))
    );
}

/// The runner's files are on its disk, in the directory the entrypoint
/// starts the service from.
#[test]
fn the_runners_own_files_are_in_the_disks_host_directory() {
    for path in [HOST_BIN, ENV_FILE, guest::CLOUD_HOST_SERVICE] {
        assert!(
            path.starts_with(&format!("{}/", guest::CLOUD_HOST_DIR)),
            "{path}"
        );
    }
}

/// The start script loads the credentials, finds the CLIs where the
/// entrypoint binds them, and runs the host with its data on the disk, the
/// relay and the name, quoted, again whenever it exits.
#[test]
fn the_start_script_runs_the_host_with_its_data_on_the_disk() {
    let script = setup::start_script("wss://relay.example", "Alex's runner");
    assert!(script.starts_with("#!/bin/sh\n"), "{script}");
    assert!(script.contains("export HOME=/home/agent "), "{script}");
    assert!(
        script.contains("for bin in /opt/fletch/tools/*/bin; do"),
        "{script}"
    );
    assert!(script.contains("\n. /data/host/env\n"), "{script}");
    assert!(
        script.contains(
            "  /data/host/fletch-host serve --data-dir /home/agent/fletch-host --relay \
             wss://relay.example --name 'Alex'\\''s runner'\n"
        ),
        "{script}"
    );
    assert!(script.contains("while :; do"), "{script}");
    assert!(
        script.find("set -a").unwrap() < script.find("serve").unwrap(),
        "the credentials are loaded before the host starts"
    );
}

/// The token is the env file's content, quoted for the shell that exports
/// it; a blank one, or one that is not one line, is refused without being
/// quoted back.
#[test]
fn the_claude_token_is_one_quoted_line_and_never_in_an_error() {
    assert_eq!(
        setup::env_file("sk-ant-oat01-abc_DEF"),
        "CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-abc_DEF\n"
    );
    assert_eq!(
        setup::env_file("it's"),
        "CLAUDE_CODE_OAUTH_TOKEN='it'\\''s'\n"
    );
    assert_eq!(
        setup::check_token("  sk-ant-oat01-abc \n").unwrap(),
        "sk-ant-oat01-abc"
    );
    for bad in ["", "  \n", "sk-ant-secret\nsecond"] {
        let refusal = setup::check_token(bad).unwrap_err().to_string();
        assert!(!refusal.contains("secret"), "{refusal}");
    }
}

/// The release the guest downloads, and the digest the engine reads from its
/// `.sha256` beside it.
#[test]
fn the_host_comes_from_this_versions_release_checked_against_its_digest() {
    assert_eq!(
        setup::release_url("0.8.29", "x86_64-unknown-linux-gnu"),
        "https://github.com/fwdai/fletch/releases/download/v0.8.29/\
         fletch-host-0.8.29-x86_64-unknown-linux-gnu.tar.gz"
    );
    let digest = "a".repeat(64);
    assert_eq!(
        setup::digest_of(&format!(
            "{}  fletch-host-0.8.29-x86_64-unknown-linux-gnu.tar.gz\n",
            digest.to_uppercase()
        ))
        .unwrap(),
        digest
    );
    for bad in ["", "not-a-digest  file", &"a".repeat(63)] {
        assert!(setup::digest_of(bad).is_err(), "{bad:?}");
    }
}

/// A device key is checked before any machine is made for it.
#[test]
fn a_device_key_that_is_not_one_is_refused_up_front() {
    let key = fletch_proto::keys::encode_key(&[7u8; 32]);
    assert_eq!(key.len(), 43);
    assert!(check_device_key(&key).is_ok());
    for bad in ["", "short", &format!("{key}="), &key.replace('A', "+")] {
        if bad == key {
            continue;
        }
        assert!(check_device_key(bad).is_err(), "{bad:?}");
    }
}

/// Every machine state is a runner state; what is left of a box with no disk
/// is a runner whose disk is lost.
#[test]
fn every_machine_state_is_a_runner_state() {
    for (machine, runner) in [
        (MachineState::Starting, RunnerState::Starting),
        (MachineState::Running, RunnerState::Running),
        (MachineState::Stopping, RunnerState::Stopping),
        (MachineState::Stopped, RunnerState::Stopped),
        (MachineState::MachineGone, RunnerState::MachineLost),
        (MachineState::DiskGone, RunnerState::DiskLost),
        (MachineState::Gone, RunnerState::DiskLost),
        (
            MachineState::Failed("host down".into()),
            RunnerState::Failed {
                reason: "host down".into(),
            },
        ),
    ] {
        assert_eq!(state_of(machine), runner);
    }
}

/// What a caller renders: the state's name and its fields beside the
/// provider and the record.
#[test]
fn a_status_serializes_flat() {
    let status = RunnerStatus {
        state: RunnerState::Failed {
            reason: "host down".into(),
        },
        provider: Some("acme"),
        record: Some(record()),
    };
    assert_eq!(
        serde_json::to_value(&status).unwrap(),
        serde_json::json!({
            "state": "failed",
            "reason": "host down",
            "provider": "acme",
            "record": {
                "hostId": record().host_id,
                "name": "Fletch runner",
                "relay": "wss://relay.example",
            },
        })
    );
    assert_eq!(
        serde_json::to_value(Progress::GithubLogin {
            user_code: "ABCD-1234".into(),
            verification_url: "https://github.com/login/device".into(),
            expires_in: 900,
        })
        .unwrap(),
        serde_json::json!({
            "step": "github_login",
            "userCode": "ABCD-1234",
            "verificationUrl": "https://github.com/login/device",
            "expiresIn": 900,
        })
    );
}

/// The directory sits beside the boxes root, never in it, under the same
/// per-build split.
#[test]
fn the_runners_directory_is_beside_the_boxes_root() {
    let dir = dir().unwrap();
    let boxes = boxes::boxes_root().unwrap();
    assert_eq!(dir.parent(), boxes.parent());
    assert!(!dir.starts_with(&boxes));
    assert_eq!(dir.file_name().unwrap(), "cloud-runner");
}

#[tokio::test]
async fn a_created_runner_keeps_its_machine_its_key_and_its_record() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let dir = runner_dir(&root);
    let heard = Heard::default();
    let configured = Mutex::new(None);

    let kept = create_in(
        &dir,
        |_| Ok(fake.client()),
        ProviderId::ALL[0],
        &heard.hear(),
        |machine| {
            *configured.lock() = Some((machine.handle.clone(), machine.spec.keep_running));
            async { Ok(record()) }
        },
    )
    .await
    .unwrap();

    assert_eq!(kept, record());
    assert_eq!(fake.acts(), ["create", "exec"], "made, its host key read");
    let (handle, keep_running) = configured.lock().clone().unwrap();
    assert!(keep_running);
    assert_eq!(boxes_cloud::read_handle(&dir).unwrap(), Some(handle));
    assert!(dir.join(channel::KEY_FILE).is_file());
    assert!(dir.join(channel::KNOWN_HOSTS_FILE).is_file());
    assert_eq!(read_record(&dir).unwrap(), Some(kept));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(heard.0.lock().first(), Some(&Progress::CreatingMachine));

    let status = status_in(&dir, |_| Ok(fake.client())).await.unwrap();
    assert_eq!(status.state, RunnerState::Running);
    assert_eq!(status.provider, Some(ProviderId::ALL[0].as_str()));
}

/// One runner per install: a create is refused while a runner exists, set
/// up or not, and makes nothing.
#[tokio::test]
async fn a_create_is_refused_while_a_runner_exists() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let dir = runner_dir(&root);
    made(&fake, &dir).await;
    std::fs::remove_file(dir.join(RECORD_FILE)).unwrap();

    let err = create_in(
        &dir,
        |_| Ok(fake.client()),
        ProviderId::ALL[0],
        &|_| {},
        |_| async { panic!("nothing is set up") },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("already"), "{err}");
    assert_eq!(fake.boxes(), 1);
    assert!(boxes_cloud::is_cloud_dir(&dir), "the runner is untouched");
}

/// A set-up that fails destroys the machine and its disk and removes the
/// directory, so a retry starts clean.
#[tokio::test]
async fn a_failed_set_up_destroys_what_was_made_and_a_retry_starts_clean() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let dir = runner_dir(&root);

    let err = create_in(
        &dir,
        |_| Ok(fake.client()),
        ProviderId::ALL[0],
        &|_| {},
        |_| async { Err(Error::Other("the GitHub code lapsed".into())) },
    )
    .await
    .unwrap_err();

    assert_eq!(err.to_string(), "the GitHub code lapsed");
    assert_eq!(fake.acts(), ["create", "exec", "destroy"]);
    assert_eq!(fake.boxes(), 0);
    assert!(!dir.exists());
    assert_eq!(
        status_in(&dir, |_| panic!("no provider is asked"))
            .await
            .unwrap()
            .state,
        RunnerState::None
    );

    made(&fake, &dir).await;
    assert_eq!(fake.boxes(), 1);
}

/// When the machine of a failed set-up cannot be destroyed, the directory
/// stays — its handle is the only way back to it — and the error says so.
#[tokio::test]
async fn a_failed_set_up_whose_machine_will_not_go_keeps_its_directory() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let dir = runner_dir(&root);
    fake.refuse_destroy();

    let err = create_in(
        &dir,
        |_| Ok(fake.client()),
        ProviderId::ALL[0],
        &|_| {},
        |_| async { Err(Error::Other("the runner refused add_device".into())) },
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(err.starts_with("the runner refused add_device — "), "{err}");
    assert!(err.contains("could not be deleted"), "{err}");
    assert!(boxes_cloud::is_cloud_dir(&dir));
    assert_eq!(
        status_in(&dir, |_| Ok(fake.client())).await.unwrap().state,
        RunnerState::Incomplete
    );
}

#[tokio::test]
async fn status_reports_each_state_of_the_machine_and_nothing_without_one() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let dir = runner_dir(&root);
    let status = status_in(&dir, |_| panic!("no provider is asked"))
        .await
        .unwrap();
    assert_eq!(
        status,
        RunnerStatus {
            state: RunnerState::None,
            provider: None,
            record: None,
        }
    );

    made(&fake, &dir).await;
    for (shape, state) in [
        (Shape::Stopped, RunnerState::Stopped),
        (Shape::Running, RunnerState::Running),
        (Shape::MachineGone, RunnerState::MachineLost),
        (Shape::DiskGone, RunnerState::DiskLost),
    ] {
        fake.shape(shape);
        let status = status_in(&dir, |_| Ok(fake.client())).await.unwrap();
        assert_eq!(status.state, state, "{shape:?}");
        assert_eq!(status.record, Some(record()));
    }
}

#[tokio::test]
async fn start_starts_a_stopped_runner_and_stop_stops_it() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let dir = runner_dir(&root);
    made(&fake, &dir).await;

    stop_in(&dir, |_| Ok(fake.client())).await.unwrap();
    assert_eq!(fake.acts(), ["create", "exec", "stop"]);
    start_in(&dir, |_| Ok(fake.client())).await.unwrap();
    assert_eq!(
        fake.acts(),
        ["create", "exec", "stop", "start", "exec"],
        "started, and its host key pinned again"
    );
}

/// A runner whose machine was lost gets a new one on its disk: the same
/// runner, its new handle kept.
#[tokio::test]
async fn start_replaces_a_lost_machine_on_the_runners_disk() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let dir = runner_dir(&root);
    made(&fake, &dir).await;
    let before = boxes_cloud::read_handle(&dir).unwrap().unwrap();

    fake.shape(Shape::MachineGone);
    start_in(&dir, |_| Ok(fake.client())).await.unwrap();

    let after = boxes_cloud::read_handle(&dir).unwrap().unwrap();
    assert_ne!(after, before);
    assert!(after.same_box(&before), "on the same disk");
    assert_eq!(fake.acts(), ["create", "exec", "replace", "exec"]);
}

/// A runner whose disk is gone is refused, in the runner's words, and never
/// made again on an empty disk.
#[tokio::test]
async fn start_refuses_a_runner_whose_disk_is_gone_and_makes_nothing() {
    for shape in [Shape::DiskGone, Shape::Gone] {
        let fake = FakeCloud::start().await;
        let root = tempfile::tempdir().unwrap();
        let dir = runner_dir(&root);
        made(&fake, &dir).await;

        fake.shape(shape);
        let err = start_in(&dir, |_| Ok(fake.client())).await.unwrap_err();
        assert!(
            matches!(&err, Error::CloudGone(m) if m.contains("runner's") && m.contains("pairings")),
            "{shape:?}: {err}"
        );
        assert_eq!(fake.acts(), ["create", "exec"], "{shape:?}: nothing made");
        assert!(
            boxes_cloud::is_cloud_dir(&dir),
            "{shape:?}: the handle is kept"
        );
    }
}

/// A delete has the runner wind its workspaces' machines down first, then
/// destroys its own machine and disk and removes the directory.
#[tokio::test]
async fn delete_winds_the_workspaces_down_before_the_runner_goes() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let dir = runner_dir(&root);
    made(&fake, &dir).await;

    let deleted = delete_in(&dir, &|_| Ok(fake.client()), false, || async {
        assert_eq!(fake.boxes(), 1, "the runner is still there to ask");
        Ok(CloudBoxesDestroyed {
            destroyed: vec!["glacier".into()],
            failed: Vec::new(),
        })
    })
    .await
    .unwrap();

    assert_eq!(
        deleted,
        Deleted {
            destroyed: vec!["glacier".into()],
            warning: None,
        }
    );
    assert_eq!(fake.acts(), ["create", "exec", "destroy"]);
    assert_eq!(fake.boxes(), 0);
    assert!(!dir.exists());
}

/// A runner that cannot be reached is not deleted unless asked to be anyway;
/// then it goes, and the answer says its workspaces' machines may remain.
#[tokio::test]
async fn an_unreachable_runner_is_deleted_only_with_force() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let dir = runner_dir(&root);
    made(&fake, &dir).await;
    let unreachable = || async { Err(Error::CloudGone("its disk is gone".into())) };

    let err = delete_in(&dir, &|_| Ok(fake.client()), false, unreachable)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("can't reach the runner"), "{err}");
    assert!(err.contains("its disk is gone"), "{err}");
    assert_eq!(fake.acts(), ["create", "exec"], "nothing destroyed");
    assert!(boxes_cloud::is_cloud_dir(&dir));

    let deleted = delete_in(&dir, &|_| Ok(fake.client()), true, unreachable)
        .await
        .unwrap();
    assert!(deleted.destroyed.is_empty());
    let warning = deleted.warning.expect("a warning");
    assert!(warning.contains("may remain"), "{warning}");
    assert!(warning.contains(ProviderId::ALL[0].label()), "{warning}");
    assert_eq!(fake.boxes(), 0);
    assert!(!dir.exists());
}

/// A runner that could not destroy all its workspaces' machines is not
/// deleted unless asked to be anyway, and then names those that may remain.
#[tokio::test]
async fn a_wind_down_that_left_machines_needs_force() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let dir = runner_dir(&root);
    made(&fake, &dir).await;
    let partly = || async {
        Ok(CloudBoxesDestroyed {
            destroyed: vec!["glacier".into()],
            failed: vec![CloudBoxNotDestroyed {
                agent_id: "otter".into(),
                error: "the provider is unavailable".into(),
            }],
        })
    };

    let err = delete_in(&dir, &|_| Ok(fake.client()), false, partly)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("otter (the provider is unavailable)"), "{err}");
    assert_eq!(fake.boxes(), 1);

    let deleted = delete_in(&dir, &|_| Ok(fake.client()), true, partly)
        .await
        .unwrap();
    assert_eq!(deleted.destroyed, ["glacier"]);
    assert!(deleted.warning.unwrap().contains("otter"));
    assert!(!dir.exists());
}

/// A directory with no machine in it (a create that never made one) is
/// removed with nothing asked of a provider or a runner.
#[tokio::test]
async fn deleting_a_runner_with_no_machine_removes_its_directory() {
    let root = tempfile::tempdir().unwrap();
    let dir = runner_dir(&root);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(RECORD_FILE), "{}").unwrap();
    let deleted = delete_in(
        &dir,
        &|_| -> Result<Client> { panic!("no provider is asked") },
        false,
        || async { panic!("no runner is asked") },
    )
    .await
    .unwrap();
    assert_eq!(deleted, Deleted::default());
    assert!(!dir.exists());
}

/// The commands the desktop calls these from run them on its async runtime,
/// which needs their futures `Send`.
#[test]
fn every_change_is_a_send_future() {
    fn send<T: Send>(_: T) {}
    let db: DbState = Arc::new(Mutex::new(rusqlite::Connection::open_in_memory().unwrap()));
    let inputs = CreateInputs {
        device_public_key: String::new(),
        device_name: None,
        device_platform: None,
        claude_token: String::new(),
        name: None,
    };
    send(create(&db, ProviderId::ALL[0], inputs, &|_| {}));
    send(status(&db));
    send(start(&db));
    send(stop(&db));
    send(delete(&db, false));
    send(pairing_link(&db, None));
}
