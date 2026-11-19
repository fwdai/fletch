//! Spares against a provider account in memory (`cloud::fake`): what
//! preparing one makes, takes and leaves; what a take moves; what a
//! replenish keeps, makes and destroys. The guest is a stand-in: nothing is
//! reached over ssh, and the installs it is asked for are recorded.

use std::sync::Barrier;

use super::*;
use crate::cloud::fake::FakeCloud;

/// The build a test's spares are made for at `fake` and its boxes want: any
/// values do, as long as both sides are given the same.
fn build(fake: &FakeCloud, clis: &[(&str, &str)]) -> Build {
    Build {
        provider: fake.client().provider().as_str().to_string(),
        image: "registry.test/fletch-cloud:1".into(),
        cpus: 2,
        memory_mib: 2048,
        disk_gib: 8,
        clis: clis
            .iter()
            .map(|(cli, version)| (cli.to_string(), version.to_string()))
            .collect(),
    }
}

/// A guest that answers without ssh: its channel reaches nothing (so a
/// halt's `sync` fails and the park goes ahead, as with a guest that does
/// not answer), and each install is recorded, the one named in `refuse`
/// failing.
#[derive(Default)]
struct FakeGuest {
    installed: Mutex<Vec<String>>,
    refuse: Option<&'static str>,
}

impl Guest for FakeGuest {
    async fn reach(&self, dir: &Path, _key: &Path, _handle: &Handle) -> Result<Channel> {
        Ok(Channel::open(dir))
    }

    async fn install(&self, _machine: &CloudMachine, _channel: &Channel, cli: &str) -> Result<()> {
        self.installed.lock().push(cli.to_string());
        match self.refuse {
            Some(refused) if refused == cli => {
                Err(Error::Other(format!("installing {cli} failed")))
            }
            _ => Ok(()),
        }
    }
}

/// The spare directories in `root`, by name.
fn spares(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = found_in(root).into_iter().map(|f| f.dir).collect();
    dirs.sort();
    dirs
}

/// A box's identity at its own paths, as a workspace's would be.
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

#[tokio::test]
async fn preparing_a_spare_makes_its_box_installs_its_clis_and_parks_it_ready() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let guest = FakeGuest::default();
    let wanted = build(&fake, &[("claude", "2.1.0"), ("codex", "0.9.0")]);

    let dir = prepare(root.path(), &wanted, &|_| Ok(fake.client()), &guest)
        .await
        .unwrap();

    assert_eq!(fake.acts(), ["create", "exec", "park"]);
    assert_eq!(*guest.installed.lock(), ["claude", "codex"]);
    assert_eq!(spares(root.path()), std::slice::from_ref(&dir));
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with(ID_PREFIX), "{name}");
    assert!(cloud::is_cloud_dir(&dir), "its handle is kept");
    for file in [
        fletch_box::channel::KEY_FILE,
        fletch_box::channel::KNOWN_HOSTS_FILE,
    ] {
        assert!(dir.join(file).is_file(), "{file}");
    }
    assert_eq!(read_built(&dir), Some(wanted), "marked ready, and for what");
    assert!(!dir.join(format!("{READY_FILE}.building")).exists());
}

/// A preparation that fails destroys the box it made and removes its
/// directory; one whose box cannot be destroyed keeps the directory, the
/// only way back to it.
#[tokio::test]
async fn a_preparation_that_fails_destroys_what_it_made() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let guest = FakeGuest {
        refuse: Some("codex"),
        ..FakeGuest::default()
    };
    let wanted = build(&fake, &[("claude", "2.1.0"), ("codex", "0.9.0")]);

    let err = prepare(root.path(), &wanted, &|_| Ok(fake.client()), &guest)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("installing codex failed"), "{err}");
    assert_eq!(fake.acts(), ["create", "exec", "destroy"], "never parked");
    assert_eq!(fake.boxes(), 0);
    assert!(spares(root.path()).is_empty());

    let unreachable = FakeCloud::start().await;
    unreachable.refuse_destroy();
    prepare(root.path(), &wanted, &|_| Ok(unreachable.client()), &guest)
        .await
        .unwrap_err();
    let kept = spares(root.path());
    assert_eq!(kept.len(), 1);
    assert!(cloud::is_cloud_dir(&kept[0]), "kept with its handle");
    assert_eq!(read_built(&kept[0]), None, "and never marked ready");
    assert_eq!(unreachable.boxes(), 1);
}

/// A spare whose provider is disconnected while it is made is not marked
/// ready: the disconnect destroyed the provider's spares before it was one,
/// so it destroys itself, through the client it was made with.
#[tokio::test]
async fn a_spare_made_while_its_provider_is_disconnected_destroys_itself() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let connected = AtomicBool::new(true);
    let client_for = |provider: ProviderId| -> Result<Client> {
        if connected.swap(false, Ordering::SeqCst) {
            Ok(fake.client())
        } else {
            Err(Error::SandboxUnavailable(cloud::not_connected(provider)))
        }
    };

    let err = prepare(
        root.path(),
        &build(&fake, &[]),
        &client_for,
        &FakeGuest::default(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("was disconnected"), "{err}");
    assert_eq!(fake.acts(), ["create", "exec", "park", "destroy"]);
    assert_eq!(fake.boxes(), 0);
    assert!(spares(root.path()).is_empty());
}

/// A new box asks for a replenish whether or not there was a spare to take,
/// so an empty spares root gets its first spares from the first spawn; a box
/// something was made for asks for none.
#[test]
fn a_take_with_no_spare_still_asks_for_a_replenish() {
    let root = tempfile::tempdir().unwrap();
    let boxes = tempfile::tempdir().unwrap();
    let wanted = Build {
        provider: ProviderId::ALL[0].as_str().to_string(),
        image: "registry.test/fletch-cloud:1".into(),
        cpus: 2,
        memory_mib: 2048,
        disk_gib: 8,
        clis: BTreeMap::new(),
    };
    let asked = std::cell::Cell::new(0);
    let new_box = boxes.path().join("glacier");

    let taken = take_then(Ok(root.path().to_path_buf()), &new_box, &wanted, || {
        asked.set(asked.get() + 1)
    });
    assert!(!taken);
    assert_eq!(asked.get(), 1, "an empty root");

    let missing = take_then(
        Err(Error::Other("no home".into())),
        &new_box,
        &wanted,
        || asked.set(asked.get() + 1),
    );
    assert!(!missing);
    assert_eq!(asked.get(), 2, "no root at all");

    std::fs::create_dir_all(&new_box).unwrap();
    std::fs::write(new_box.join("key"), "a key").unwrap();
    take_then(Ok(root.path().to_path_buf()), &new_box, &wanted, || {
        asked.set(asked.get() + 1)
    });
    assert_eq!(asked.get(), 2, "a box made already");
}

/// A new box takes a ready spare: the spare's directory becomes the box's
/// with its marker gone, so the box's boot wakes the spare's machine and
/// makes none; the next replenish makes the replacement.
#[tokio::test]
async fn a_new_box_takes_a_ready_spare_and_a_replacement_is_prepared() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let boxes = tempfile::tempdir().unwrap();
    let guest = FakeGuest::default();
    let wanted = build(&fake, &[("claude", "2.1.0")]);
    let client_for = |_: ProviderId| -> Result<Client> { Ok(fake.client()) };

    replenish_with(
        root.path(),
        std::slice::from_ref(&wanted),
        1,
        &client_for,
        &guest,
    )
    .await;
    let [spare] = &spares(root.path())[..] else {
        panic!("one spare");
    };
    let spare = spare.clone();
    let handle = cloud::read_handle(&spare).unwrap().unwrap();

    let box_dir = boxes.path().join("glacier");
    assert!(take_in(root.path(), &box_dir, &wanted));
    assert!(!spare.exists(), "moved, not copied");
    assert!(spares(root.path()).is_empty());
    assert_eq!(cloud::read_handle(&box_dir).unwrap(), Some(handle.clone()));
    assert!(box_dir.join(fletch_box::channel::KEY_FILE).is_file());
    assert!(!box_dir.join(READY_FILE).exists(), "no longer a spare");

    let woken = cloud::machine_up(
        &Arc::new(fake.client()),
        &box_dir,
        &cloud::spec(&identity("glacier"), "ssh-ed25519 AAAA engine".into()),
        &|_| {},
    )
    .await
    .unwrap();
    assert_eq!(woken, handle, "the spare's own machine");
    assert_eq!(
        fake.acts(),
        ["create", "exec", "park", "start"],
        "woken, not made, its pinned host key trusted"
    );

    replenish_with(root.path(), &[wanted], 1, &client_for, &guest).await;
    assert_eq!(spares(root.path()).len(), 1, "a replacement");
    assert_eq!(
        fake.acts()[4..],
        ["create", "exec", "park"],
        "made and parked"
    );
    assert_eq!(fake.boxes(), 2);
}

/// With no spare fit to take the box is made directly: none ready, one made
/// for another build, or a box directory that holds something already.
#[tokio::test]
async fn with_no_spare_ready_the_box_is_made_directly() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let boxes = tempfile::tempdir().unwrap();
    let guest = FakeGuest::default();
    let wanted = build(&fake, &[("claude", "2.1.0")]);
    let box_dir = boxes.path().join("glacier");

    assert!(!take_in(root.path(), &box_dir, &wanted), "no spare at all");

    let older = build(&fake, &[("claude", "2.0.9")]);
    let spare = prepare(root.path(), &older, &|_| Ok(fake.client()), &guest)
        .await
        .unwrap();
    let other_image = Build {
        image: "registry.test/fletch-cloud:2".into(),
        ..older.clone()
    };
    assert!(!take_in(root.path(), &box_dir, &other_image));
    assert!(read_built(&spare).is_some(), "left as it was");

    std::fs::create_dir_all(&box_dir).unwrap();
    std::fs::write(box_dir.join("key"), "a key of the box's own").unwrap();
    assert!(
        !take_in(root.path(), &box_dir, &older),
        "a box something was made for"
    );
    assert!(read_built(&spare).is_some());

    let made = cloud::machine_up(
        &Arc::new(fake.client()),
        &box_dir,
        &cloud::spec(&identity("glacier"), "ssh-ed25519 AAAA engine".into()),
        &|_| {},
    )
    .await
    .unwrap();
    assert_ne!(Some(made), cloud::read_handle(&spare).unwrap());
    assert_eq!(fake.acts()[3..], ["create", "exec"], "made directly");

    // An empty directory is one nothing was made for, and a spare whose CLI
    // is another version is still the machine: the launch brings its CLI up.
    let empty = boxes.path().join("shasta");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(take_in(root.path(), &empty, &wanted));
    assert!(!spare.exists());
}

/// A replenish destroys a stale spare, one a preparation left unfinished
/// with a box at the provider, and one left with nothing made; a destroy that
/// fails keeps the directory. It keeps a fresh spare, and destroys spares
/// past the number asked for.
#[tokio::test]
async fn a_stale_spare_and_a_half_prepared_one_are_destroyed() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let guest = FakeGuest::default();
    let client_for = |_: ProviderId| -> Result<Client> { Ok(fake.client()) };
    let older = build(&fake, &[("claude", "2.0.9")]);
    let wanted = build(&fake, &[("claude", "2.1.0")]);

    let stale = prepare(root.path(), &older, &client_for, &guest)
        .await
        .unwrap();
    let unfinished = prepare(root.path(), &wanted, &client_for, &guest)
        .await
        .unwrap();
    std::fs::remove_file(unfinished.join(READY_FILE)).unwrap();
    let nothing_made = root.path().join("spare-000000000000");
    std::fs::create_dir_all(&nothing_made).unwrap();
    std::fs::write(nothing_made.join("key"), "a key").unwrap();
    let not_a_spare = root.path().join("notes");
    std::fs::create_dir_all(&not_a_spare).unwrap();
    assert_eq!(fake.boxes(), 2);

    replenish_with(
        root.path(),
        std::slice::from_ref(&wanted),
        0,
        &client_for,
        &guest,
    )
    .await;
    assert!(spares(root.path()).is_empty());
    assert!(!stale.exists() && !unfinished.exists() && !nothing_made.exists());
    assert!(not_a_spare.is_dir(), "what is not a spare is left alone");
    assert_eq!(fake.boxes(), 0);
    assert_eq!(
        fake.acts().iter().filter(|act| **act == "destroy").count(),
        2
    );

    // Two fresh ones, one asked for: one is destroyed, one kept as it is.
    for _ in 0..2 {
        prepare(root.path(), &wanted, &client_for, &guest)
            .await
            .unwrap();
    }
    replenish_with(
        root.path(),
        std::slice::from_ref(&wanted),
        1,
        &client_for,
        &guest,
    )
    .await;
    let kept = spares(root.path());
    assert_eq!(kept.len(), 1);
    assert_eq!(read_built(&kept[0]), Some(wanted.clone()));
    assert_eq!(fake.boxes(), 1);

    // A provider that cannot destroy it: the stale spare's directory stays.
    let unreachable = FakeCloud::start().await;
    let stale = prepare(root.path(), &older, &|_| Ok(unreachable.client()), &guest)
        .await
        .unwrap();
    unreachable.refuse_destroy();
    replenish_with(
        root.path(),
        &[wanted],
        0,
        &|_| Ok(unreachable.client()),
        &guest,
    )
    .await;
    assert!(cloud::is_cloud_dir(&stale), "kept with its handle");
    assert_eq!(read_built(&stale), None, "and never taken");
    assert_eq!(unreachable.boxes(), 1);
}

/// A replenish with no provider connected does nothing at all.
#[tokio::test]
async fn a_replenish_with_no_provider_connected_does_nothing() {
    let root = tempfile::tempdir().unwrap();
    let left = root.path().join("spare-000000000000");
    std::fs::create_dir_all(&left).unwrap();
    replenish_with(
        root.path(),
        &[],
        3,
        &|_| panic!("no provider is asked"),
        &FakeGuest::default(),
    )
    .await;
    assert!(left.is_dir());
}

/// The spares are beside the boxes, never among them: the sweep of boxes no
/// workspace owns lists the boxes root alone, where a spare is not.
#[test]
fn the_orphan_sweep_leaves_the_spares_root_alone() {
    let spares_root = root().unwrap();
    let boxes_root = super::super::boxes_root().unwrap();
    assert_eq!(spares_root.parent(), boxes_root.parent(), "one volume");
    assert!(!spares_root.starts_with(&boxes_root));

    let base = tempfile::tempdir().unwrap();
    let boxes = base.path().join("boxes");
    let spare = boxes
        .with_file_name("box-spares")
        .join("spare-0123456789ab");
    std::fs::create_dir_all(&spare).unwrap();
    std::fs::create_dir_all(&boxes).unwrap();
    std::fs::write(spare.join(cloud::HANDLE_FILE), "a handle\n").unwrap();
    assert!(super::super::boxes_in(&boxes).is_empty());
}

/// Two boxes taking at once get one spare between them: the other is made
/// directly.
#[tokio::test]
async fn two_takers_get_one_spare_between_them() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let boxes = tempfile::tempdir().unwrap();
    let wanted = build(&fake, &[("claude", "2.1.0")]);
    prepare(
        root.path(),
        &wanted,
        &|_| Ok(fake.client()),
        &FakeGuest::default(),
    )
    .await
    .unwrap();

    let start = Arc::new(Barrier::new(2));
    let takers: Vec<_> = ["glacier", "shasta"]
        .into_iter()
        .map(|id| {
            let (root, box_dir) = (root.path().to_path_buf(), boxes.path().join(id));
            let (start, wanted) = (start.clone(), wanted.clone());
            std::thread::spawn(move || {
                start.wait();
                take_in(&root, &box_dir, &wanted)
            })
        })
        .collect();
    let took: Vec<bool> = takers.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(took.iter().filter(|t| **t).count(), 1, "{took:?}");
    assert!(spares(root.path()).is_empty());
}

/// Destroying every spare of a provider takes them from it and removes their
/// directories; one that cannot be destroyed is kept, and said so.
#[tokio::test]
async fn every_spare_of_a_provider_is_destroyed_on_demand() {
    let fake = FakeCloud::start().await;
    let root = tempfile::tempdir().unwrap();
    let guest = FakeGuest::default();
    let client_for = |_: ProviderId| -> Result<Client> { Ok(fake.client()) };
    let wanted = build(&fake, &[]);
    for _ in 0..2 {
        prepare(root.path(), &wanted, &client_for, &guest)
            .await
            .unwrap();
    }
    let nothing_made = root.path().join("spare-000000000000");
    std::fs::create_dir_all(&nothing_made).unwrap();

    destroy_all_in(root.path(), Some(fake.client().provider()), &client_for)
        .await
        .unwrap();
    assert_eq!(fake.boxes(), 0);
    assert_eq!(
        spares(root.path()),
        std::slice::from_ref(&nothing_made),
        "no provider's"
    );
    destroy_all_in(root.path(), None, &client_for)
        .await
        .unwrap();
    assert!(spares(root.path()).is_empty());

    prepare(root.path(), &wanted, &client_for, &guest)
        .await
        .unwrap();
    fake.refuse_destroy();
    let err = destroy_all_in(root.path(), None, &client_for)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("kept"), "{err}");
    assert_eq!(spares(root.path()).len(), 1);
}

/// A replenish asked for while one runs does not run beside it: the running
/// one goes once more when it is done, and the next call runs afresh.
#[tokio::test]
async fn a_replenish_runs_one_at_a_time() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let flight = Mutex::new(Flight::new());
    let (runs, running, most) = (
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
    );
    let work = || async {
        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
        most.fetch_max(now, Ordering::SeqCst);
        tokio::task::yield_now().await;
        runs.fetch_add(1, Ordering::SeqCst);
        running.fetch_sub(1, Ordering::SeqCst);
    };

    tokio::join!(
        single_flight(&flight, work),
        single_flight(&flight, work),
        single_flight(&flight, work),
    );
    assert_eq!(runs.load(Ordering::SeqCst), 2, "the first, and once more");
    assert_eq!(most.load(Ordering::SeqCst), 1, "never two at once");

    single_flight(&flight, work).await;
    assert_eq!(runs.load(Ordering::SeqCst), 3);
}
