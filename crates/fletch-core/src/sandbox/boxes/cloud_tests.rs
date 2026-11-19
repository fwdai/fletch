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
    assert_eq!(fake.acts(), ["create", "exec", "start", "exec"]);

    let again = machine_up(&client, dir.path(), &spec, &Phases::default().hear())
        .await
        .unwrap();
    assert_eq!(again, handle);
    assert_eq!(
        fake.acts(),
        ["create", "exec", "start", "exec", "exec"],
        "running: nothing but the host key read again"
    );
    assert_eq!(fake.boxes(), 1);
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
    assert_eq!(fake.acts(), ["create", "exec", "replace", "exec"]);
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

/// A handle that is not one is an error, never a box on this machine.
#[test]
fn a_damaged_handle_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(HANDLE_FILE), "not a handle\n").unwrap();
    assert!(read_handle(dir.path()).is_err());
}
