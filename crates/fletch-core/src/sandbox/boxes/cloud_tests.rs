//! The provider half of a cloud box's boot against a provider account in
//! memory (`cloud::fake`): what each state of a box makes a boot do, what is
//! kept in the box directory, and that a box whose disk is gone is never
//! made again. No ssh: the guest is reached only through the provider here.

use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use crate::cloud::fake::{FakeCloud, Shape, HOST_KEY};

fn identity(id: &str) -> BoxIdentity {
    BoxIdentity {
        agent_id: id.into(),
        project_id: "p".into(),
        provider: "claude".into(),
        writable_root: PathBuf::from(format!("/Users/u/.fletch/workspaces/{id}")),
        rpc_dir: PathBuf::from(format!("/Users/u/.fletch/rpc/{id}")),
        home: PathBuf::from("/Users/u"),
        shares: Vec::new(),
    }
}

/// A boot's phases, as the registry would report them.
#[derive(Default)]
struct Phases(Mutex<Vec<BoxPhase>>);

impl Phases {
    fn hear(&self) -> impl Fn(BoxPhase) + Sync + '_ {
        move |phase| self.0.lock().push(phase)
    }

    fn details(&self) -> Vec<String> {
        self.0
            .lock()
            .iter()
            .filter_map(|phase| match phase {
                BoxPhase::Preparing { detail } => Some(detail.clone()),
                _ => None,
            })
            .collect()
    }
}

/// A box directory with a made box in it: `machine_up` from nothing.
async fn made(fake: &FakeCloud, dir: &Path) -> Handle {
    let client = Arc::new(fake.client());
    machine_up(
        &client,
        dir,
        &spec(&identity("glacier"), "ssh-ed25519 AAAA engine".into()),
        &Phases::default().hear(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_new_box_is_made_its_handle_kept_at_once_and_its_host_key_pinned() {
    let fake = FakeCloud::start().await;
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(fake.client());
    let phases = Phases::default();
    let spec = spec(&identity("glacier"), "ssh-ed25519 AAAA engine".into());

    let handle = machine_up(&client, dir.path(), &spec, &phases.hear())
        .await
        .unwrap();

    assert_eq!(fake.acts(), ["create", "exec"]);
    assert_eq!(read_handle(dir.path()).unwrap(), Some(handle.clone()));
    assert!(is_cloud_dir(dir.path()));
    let kept = dir.path().join(HANDLE_FILE);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777,
        0o600,
        "the handle is the owner's alone"
    );
    let known_hosts =
        std::fs::read_to_string(dir.path().join(fletch_box::channel::KNOWN_HOSTS_FILE)).unwrap();
    assert_eq!(
        known_hosts,
        HostKey::from_public_key(HOST_KEY)
            .unwrap()
            .known_hosts_line()
    );
    assert_eq!(
        phases.details(),
        [format!("creating a {} machine", client.provider().label())]
    );
}

#[tokio::test]
async fn a_stopped_machine_is_started_and_a_running_one_left_alone() {
    let fake = FakeCloud::start().await;
    let dir = tempfile::tempdir().unwrap();
    let handle = made(&fake, dir.path()).await;
    let client = Arc::new(fake.client());
    let spec = spec(&identity("glacier"), "ssh-ed25519 AAAA engine".into());

    fake.shape(Shape::Stopped);
    let again = machine_up(&client, dir.path(), &spec, &Phases::default().hear())
        .await
        .unwrap();
    assert_eq!(again, handle);
    assert_eq!(
        fake.acts(),
        ["create", "exec", "start"],
        "its host key is pinned already"
    );

    let again = machine_up(&client, dir.path(), &spec, &Phases::default().hear())
        .await
        .unwrap();
    assert_eq!(again, handle);
    assert_eq!(fake.acts(), ["create", "exec", "start"], "running: nothing");
    assert_eq!(fake.boxes(), 1);
}

/// A parked machine is at rest like a stopped one, and a boot wakes it the
/// same way: started, on the same handle, its pinned host key trusted.
#[tokio::test]
async fn a_parked_machine_is_woken_as_a_stopped_one_is() {
    let fake = FakeCloud::start().await;
    let dir = tempfile::tempdir().unwrap();
    let handle = made(&fake, dir.path()).await;
    let client = Arc::new(fake.client());

    client.park(&handle).await.unwrap();
    assert_eq!(client.state(&handle).await.unwrap(), MachineState::Stopped);
    client.park(&handle).await.unwrap();
    assert_eq!(
        fake.acts(),
        ["create", "exec", "park"],
        "a machine at rest is parked once"
    );

    let again = machine_up(
        &client,
        dir.path(),
        &spec(&identity("glacier"), "ssh-ed25519 AAAA engine".into()),
        &Phases::default().hear(),
    )
    .await
    .unwrap();
    assert_eq!(again, handle);
    assert_eq!(fake.acts(), ["create", "exec", "park", "start"]);
}

/// A made box's running machine, and a channel to a guest that never
/// answers: a stop's own sync over it fails at once.
async fn running_machine(fake: &FakeCloud, dir: &Path) -> (CloudMachine, Channel) {
    let handle = made(fake, dir).await;
    let machine = CloudMachine::new("glacier", Arc::new(fake.client()), handle);
    (machine, Channel::open(dir))
}

/// A clean stop lets a CLI record still being made finish before the
/// provider stops the machine, so the version it records is on the disk.
#[tokio::test]
async fn a_halt_lets_a_pending_cli_record_finish_before_the_machine_stops() {
    let fake = FakeCloud::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (machine, channel) = running_machine(&fake, dir.path()).await;
    let (finish, finished) = tokio::sync::oneshot::channel::<()>();
    machine.records.lock().push(tokio::spawn(async move {
        let _ = finished.await;
    }));

    let halt = machine.halt(&channel, StopReason::User, None);
    tokio::pin!(halt);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut halt)
            .await
            .is_err(),
        "the stop waits for the record"
    );
    assert_eq!(fake.acts(), ["create", "exec"], "not stopped yet");

    finish.send(()).unwrap();
    assert_eq!(halt.await, StopOutcome::Control);
    assert_eq!(fake.acts(), ["create", "exec", "stop"]);
    assert!(machine.records.lock().is_empty());
}

/// Under a bound, a record gets what the stop's own sync leaves of it, and
/// the machine is stopped whatever became of the record.
#[tokio::test]
async fn a_bounded_halt_waits_for_a_cli_record_no_longer_than_its_share() {
    let fake = FakeCloud::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (machine, channel) = running_machine(&fake, dir.path()).await;
    machine
        .records
        .lock()
        .push(tokio::spawn(std::future::pending::<()>()));

    let share = Duration::from_millis(300);
    let started = Instant::now();
    let outcome = machine
        .halt(&channel, StopReason::User, Some(SYNC_BOUND + share))
        .await;

    let waited = started.elapsed();
    assert!(waited >= share && waited < SYNC_BOUND, "{waited:?}");
    assert_eq!(outcome, StopOutcome::Control);
    assert_eq!(fake.acts(), ["create", "exec", "stop"]);
}

/// A record whose guest cannot be synced (here, cannot be reached) writes no
/// marker and ends, warned of; the stop that waits for it goes on.
#[tokio::test]
async fn a_cli_record_that_cannot_sync_ends_and_the_stop_goes_on() {
    let fake = FakeCloud::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (machine, channel) = running_machine(&fake, dir.path()).await;

    machine.record_in_background(&channel, "claude", "2.1.0");
    assert_eq!(machine.records.lock().len(), 1);

    assert_eq!(
        machine.halt(&channel, StopReason::User, None).await,
        StopOutcome::Control
    );
    assert!(machine.records.lock().is_empty());
    assert_eq!(fake.acts(), ["create", "exec", "stop"]);
}

/// A host key no box of this account has.
const OTHER_HOST_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA another-box";

/// `key` as the box directory pins it.
fn pin_of(key: &str) -> String {
    HostKey::from_public_key(key).unwrap().known_hosts_line()
}

/// The guest keeps its host key on the box's disk, so a boot trusts the pin
/// its directory holds, right or wrong (the ssh handshake checks it), and
/// reads the key through the provider only where there is none.
#[tokio::test]
async fn a_boot_reads_the_host_key_only_where_its_directory_has_no_pin() {
    let fake = FakeCloud::start().await;
    let dir = tempfile::tempdir().unwrap();
    made(&fake, dir.path()).await;
    let client = Arc::new(fake.client());
    let spec = spec(&identity("glacier"), "ssh-ed25519 AAAA engine".into());
    let known_hosts = dir.path().join(fletch_box::channel::KNOWN_HOSTS_FILE);

    std::fs::write(&known_hosts, pin_of(OTHER_HOST_KEY)).unwrap();
    fake.shape(Shape::Stopped);
    machine_up(&client, dir.path(), &spec, &Phases::default().hear())
        .await
        .unwrap();
    assert_eq!(fake.acts(), ["create", "exec", "start"]);
    assert_eq!(
        std::fs::read_to_string(&known_hosts).unwrap(),
        pin_of(OTHER_HOST_KEY)
    );

    for no_pin in ["removed", "empty"] {
        match no_pin {
            "removed" => std::fs::remove_file(&known_hosts).unwrap(),
            _ => std::fs::write(&known_hosts, "").unwrap(),
        }
        machine_up(&client, dir.path(), &spec, &Phases::default().hear())
            .await
            .unwrap();
        assert_eq!(fake.acts().last(), Some(&"exec"), "{no_pin}");
        assert_eq!(
            std::fs::read_to_string(&known_hosts).unwrap(),
            pin_of(HOST_KEY),
            "{no_pin}"
        );
    }
}

/// A box just made is a new disk with a key of its own: that key is pinned,
/// whatever the directory held before.
#[tokio::test]
async fn a_new_box_pins_its_own_host_key_over_whatever_its_directory_held() {
    let fake = FakeCloud::start().await;
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join(fletch_box::channel::KNOWN_HOSTS_FILE);
    std::fs::write(&known_hosts, pin_of(OTHER_HOST_KEY)).unwrap();

    made(&fake, dir.path()).await;

    assert_eq!(fake.acts(), ["create", "exec"]);
    assert_eq!(
        std::fs::read_to_string(&known_hosts).unwrap(),
        pin_of(HOST_KEY)
    );
}

/// A try at ssh that fails is followed by a short pause first, then longer
/// ones, up to the longest.
#[test]
fn the_pauses_between_ssh_tries_start_short_and_grow_to_the_pace() {
    let pauses: Vec<Duration> = ssh_pauses().take(7).collect();
    assert_eq!(pauses[0], SSH_FIRST_PACE);
    assert!(pauses.windows(2).all(|w| w[0] <= w[1]), "{pauses:?}");
    assert_eq!(pauses.last(), Some(&SSH_PACE), "{pauses:?}");
    assert!(pauses.iter().all(|p| *p <= SSH_PACE), "{pauses:?}");
}

/// What a launch learns of a CLI in one cloud box (its capability probe)
/// holds in every other with that version: the CLI is named by its
/// version, never by its box.
#[tokio::test]
async fn a_cloud_boxs_cli_is_named_by_its_version_alone() {
    let fake = FakeCloud::start().await;
    let dir = tempfile::tempdir().unwrap();
    let handle = made(&fake, dir.path()).await;
    let client = Arc::new(fake.client());
    let one = CloudMachine::new("glacier", client.clone(), handle.clone());
    let other = CloudMachine::new("tundra", client, handle);
    assert_eq!(one.cli_identity("claude"), None, "no version known yet");

    for machine in [&one, &other] {
        machine
            .clis
            .lock()
            .insert("claude".to_string(), "2.1.0".to_string());
    }
    assert!(one.cli_identity("claude").is_some());
    assert_eq!(one.cli_identity("claude"), other.cli_identity("claude"));

    other
        .clis
        .lock()
        .insert("claude".to_string(), "2.2.0".to_string());
    assert_ne!(one.cli_identity("claude"), other.cli_identity("claude"));
    assert_eq!(other.cli_identity("codex"), None);
}

#[tokio::test]
async fn a_lost_machine_is_replaced_on_its_disk_and_the_new_handle_kept() {
    let fake = FakeCloud::start().await;
    let dir = tempfile::tempdir().unwrap();
    let handle = made(&fake, dir.path()).await;
    let client = Arc::new(fake.client());
    let phases = Phases::default();

    fake.shape(Shape::MachineGone);
    let replaced = machine_up(
        &client,
        dir.path(),
        &spec(&identity("glacier"), "ssh-ed25519 AAAA engine".into()),
        &phases.hear(),
    )
    .await
    .unwrap();

    assert_ne!(replaced, handle, "a new machine");
    assert!(replaced.same_box(&handle), "on the same disk");
    assert_eq!(read_handle(dir.path()).unwrap(), Some(replaced));
    assert_eq!(
        fake.acts(),
        ["create", "exec", "replace"],
        "the same disk, the same host key"
    );
    assert_eq!(
        phases.details(),
        [format!(
            "replacing the {} machine",
            client.provider().label()
        )]
    );
}

#[tokio::test]
async fn a_box_whose_disk_is_gone_is_refused_and_nothing_is_made_in_its_place() {
    for shape in [Shape::DiskGone, Shape::Gone] {
        let fake = FakeCloud::start().await;
        let dir = tempfile::tempdir().unwrap();
        let handle = made(&fake, dir.path()).await;
        let client = Arc::new(fake.client());

        fake.shape(shape);
        let err = machine_up(
            &client,
            dir.path(),
            &spec(&identity("glacier"), "ssh-ed25519 AAAA engine".into()),
            &Phases::default().hear(),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(&err, Error::CloudGone(m) if m.contains("its files with it")),
            "{shape:?}: {err}"
        );
        assert_eq!(fake.acts(), ["create", "exec"], "{shape:?}: nothing made");
        assert_eq!(
            read_handle(dir.path()).unwrap(),
            Some(handle),
            "{shape:?}: the handle is kept"
        );
    }
}

#[tokio::test]
async fn destroying_a_box_takes_it_from_its_provider_and_a_dir_without_one_is_nothing() {
    let fake = FakeCloud::start().await;
    let dir = tempfile::tempdir().unwrap();
    made(&fake, dir.path()).await;
    destroy_box_with(dir.path(), |_| Ok(fake.client()))
        .await
        .unwrap();
    assert_eq!(fake.acts(), ["create", "exec", "destroy"]);
    assert_eq!(fake.boxes(), 0);
    // Destroyed already: success again.
    destroy_box_with(dir.path(), |_| Ok(fake.client()))
        .await
        .unwrap();

    let local = tempfile::tempdir().unwrap();
    destroy_box_with(local.path(), |_| {
        panic!("a box on this machine asks no provider")
    })
    .await
    .unwrap();
}

/// A new machine is made from the pinned cloud image with the guest's
/// environment — the identity binds, the box's key, the box id, the sshd
/// port — and no credential, sized as a box is.
#[test]
fn a_machine_is_made_from_the_pinned_image_with_the_guest_contract() {
    let spec = spec(&identity("glacier"), "ssh-ed25519 AAAA engine".into());
    let env: std::collections::HashMap<&str, &str> = spec
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        env,
        std::collections::HashMap::from([
            (
                "FLETCH_BOX_WRITABLE_ROOT",
                "/Users/u/.fletch/workspaces/glacier"
            ),
            ("FLETCH_BOX_RPC_DIR", "/Users/u/.fletch/rpc/glacier"),
            ("FLETCH_BOX_HOME", "/Users/u"),
            ("FLETCH_BOX_AUTHORIZED_KEY", "ssh-ed25519 AAAA engine"),
            ("FLETCH_BOX_ID", "glacier"),
            ("FLETCH_BOX_SSH_PORT", "2222"),
        ])
    );
    assert_eq!(spec.name, "glacier");
    assert_eq!(spec.image, image());
    assert_eq!(spec.ssh_port, fletch_box::guest::CLOUD_SSH_PORT);
    let knobs = settings::current();
    assert_eq!(
        (spec.cpus, spec.memory_mib, spec.disk_gib),
        (
            u32::from(knobs.cpus),
            knobs.memory_mib,
            Some(knobs.disk_gib)
        )
    );
}

/// With no database installed (every unit test) no box is a cloud box and no
/// provider is connected; a client is refused with the sentence the user
/// sees.
#[test]
fn without_a_database_nothing_is_in_the_cloud() {
    assert_eq!(backing("any-agent").unwrap(), None);
    assert_eq!(box_backing("any-agent"), None);
    assert_eq!(setup_allowance("any-agent"), None);
    for &provider in ProviderId::ALL {
        assert!(connection(provider).unwrap().is_none());
        let Err(Error::SandboxUnavailable(why)) = client(provider) else {
            panic!("a client without a connection");
        };
        assert_eq!(why, not_connected(provider));
        assert!(why.contains(provider.label()), "{why}");
        assert!(why.contains("Settings → Cloud"), "{why}");
    }
}

/// The binds a cloud boot asks of its guest, run here with `mountpoint` and
/// `sudo` standing in for the guest's. A guest whose paths are all bound (a
/// box made for its workspace, whose entrypoint bound them) runs nothing
/// privileged. A taken spare's guest, which has only the home bound (the
/// placeholder's is the same), gets the rpc dir and the writable root made,
/// the directories on the way given to the guest user, and each bound from
/// its data directory. A path the entrypoint refuses is refused here too.
#[test]
fn a_cloud_boot_binds_the_paths_its_guest_lacks_and_nothing_else() {
    use std::os::unix::fs::PermissionsExt;
    // A top-level directory that exists here and that the binds accept.
    let Some(top) = dirs::home_dir()
        .and_then(|home| home.components().nth(1).map(|c| Path::new("/").join(c)))
        .filter(|top| {
            !["/tmp", "/var", "/usr", "/opt", "/data"].contains(&&*top.to_string_lossy())
        })
    else {
        return;
    };
    let scratch = tempfile::tempdir().unwrap();
    let stubs = scratch.path().join("bin");
    std::fs::create_dir(&stubs).unwrap();
    for (name, script) in [
        (
            "mountpoint",
            "#!/bin/sh\n[ \"$1\" = -q ] && shift\ngrep -qxF -- \"$1\" \"$MOUNTED\"\n",
        ),
        (
            "sudo",
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$SUDO_LOG\"\n\
             [ \"$2\" = mount ] && printf '%s\\n' \"$5\" >> \"$MOUNTED\"\nexit 0\n",
        ),
    ] {
        let stub = stubs.join(name);
        std::fs::write(&stub, script).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (mounted, sudo_log) = (scratch.path().join("mounted"), scratch.path().join("sudo"));
    let run = |identity: &BoxIdentity, bound: &[&Path]| {
        let lines: String = bound.iter().map(|p| format!("{}\n", p.display())).collect();
        std::fs::write(&mounted, lines).unwrap();
        std::fs::write(&sudo_log, "").unwrap();
        let argv = bind_identity_argv(identity);
        let out = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .env("PATH", format!("{}:/usr/bin:/bin", stubs.display()))
            .env("MOUNTED", &mounted)
            .env("SUDO_LOG", &sudo_log)
            .output()
            .unwrap();
        let sudo: Vec<String> = std::fs::read_to_string(&sudo_log)
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
        (out.status.success(), sudo)
    };

    let home = top
        .join(format!("fletch-bind-{}", uuid::Uuid::new_v4().simple()))
        .join("u");
    let glacier = BoxIdentity {
        writable_root: home.join(".fletch/workspaces/glacier"),
        rpc_dir: home.join(".fletch/rpc/glacier"),
        home: home.clone(),
        ..identity("glacier")
    };
    let (ws, rpc) = (&glacier.writable_root, &glacier.rpc_dir);

    let (ok, sudo) = run(&glacier, &[&home, rpc, ws]);
    assert!(ok);
    assert!(sudo.is_empty(), "bound already: {sudo:?}");

    let (ok, sudo) = run(&glacier, &[&home]);
    assert!(ok, "{sudo:?}");
    let mounts: Vec<&String> = sudo.iter().filter(|c| c.starts_with("-n mount")).collect();
    assert_eq!(
        mounts,
        [
            &format!("-n mount --bind /data/rpc {}", rpc.display()),
            &format!("-n mount --bind /data/ws {}", ws.display()),
        ]
    );
    for made in [rpc, ws] {
        assert!(
            sudo.contains(&format!("-n mkdir -p {}", made.display())),
            "{sudo:?}"
        );
    }
    for owned in [
        home.join(".fletch/workspaces"),
        home.join(".fletch"),
        home.clone(),
    ] {
        assert!(
            sudo.contains(&format!("-n chown -h agent:agent {}", owned.display())),
            "{owned:?}: {sudo:?}"
        );
    }
    assert!(
        !sudo.iter().any(|c| c.contains("/data/home")),
        "the home was bound: {sudo:?}"
    );

    let shadowing = BoxIdentity {
        rpc_dir: PathBuf::from("/etc/fletch/rpc/glacier"),
        ..glacier.clone()
    };
    let (ok, sudo) = run(&shadowing, &[&home]);
    assert!(!ok, "refused");
    assert!(sudo.is_empty(), "before anything is made: {sudo:?}");
}

/// A handle that is not one is an error, never a box on this machine.
#[test]
fn a_damaged_handle_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(HANDLE_FILE), "not a handle\n").unwrap();
    assert!(read_handle(dir.path()).is_err());
}
