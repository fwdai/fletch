//! Structural deltas: pure over file lists, then landed through the merge
//! entry ([`ingest_merged`]) over `ContextStore::temp()`, seeded by the
//! bootstrap from the tree before the merge.

use super::*;
use crate::capture::ingest::{ingest_merged, read_and_ingest, seen, MergedPr};
use crate::context::{compile, ContextStore};

const PROJECT: &str = "proj-ctx";
const WORKSPACE: &str = "ws-denali";
const REPO: &str = "app";
const PR: &str = "https://github.com/o/app/pull/9";
const SHA: &str = "4f1c0de";

/// A root app and two crates: modules `src`, `src-components`, `core`,
/// `core-store` and `old`.
const BEFORE: [&str; 8] = [
    "package.json",
    "src/main.tsx",
    "src/components/App.tsx",
    "crates/core/Cargo.toml",
    "crates/core/src/lib.rs",
    "crates/core/src/store/mod.rs",
    "crates/old/Cargo.toml",
    "crates/old/src/lib.rs",
];

fn tree(paths: &[&str]) -> Vec<Module> {
    rules::modules(&paths.iter().map(|p| p.to_string()).collect::<Vec<_>>())
}

/// [`BEFORE`] without the files under `drop`, plus `add`.
fn after(drop: &[&str], add: &[&str]) -> Vec<Module> {
    let mut files: Vec<&str> = BEFORE
        .iter()
        .copied()
        .filter(|f| !drop.iter().any(|d| f.starts_with(d)))
        .collect();
    files.extend_from_slice(add);
    tree(&files)
}

fn slugs(modules: &[Module]) -> Vec<&str> {
    modules.iter().map(|m| m.slug.as_str()).collect()
}

fn service() -> (ContextService, tempfile::TempDir) {
    let (store, dir) = ContextStore::temp().unwrap();
    store.own(PROJECT, "fp-ctx");
    (
        ContextService::new(
            store.db().clone(),
            std::sync::Arc::new(crate::host::sink::NullSink),
        )
        .unwrap(),
        dir,
    )
}

fn project() -> Project {
    Project {
        id: PROJECT.into(),
        fletch_id: "fp-ctx".into(),
    }
}

/// A service whose project was bootstrapped from [`BEFORE`].
fn bootstrapped() -> (ContextService, tempfile::TempDir) {
    let (service, dir) = service();
    bootstrap::apply(
        &service,
        &project(),
        &tree(&BEFORE),
        bootstrap::stamp("base"),
    )
    .unwrap();
    (service, dir)
}

fn merge(service: &ContextService, structure: StructureDelta, body: &str) {
    let pr = MergedPr {
        reference: PR.into(),
        body: body.into(),
        branch: Some("feat/net".into()),
        sha: Some(SHA.into()),
        structure,
    };
    ingest_merged(service, &project(), WORKSPACE, REPO, true, &pr).unwrap();
}

fn graph(service: &ContextService) -> Graph {
    service.graph(&project()).unwrap()
}

fn entity<'a>(graph: &'a Graph, slug: &str) -> &'a Entity {
    graph
        .entities
        .iter()
        .find(|e| e.slug == slug)
        .unwrap_or_else(|| panic!("no entity `{slug}`"))
}

fn part_of(graph: &Graph, child: &str, parent: &str) -> bool {
    let (child, parent) = (entity(graph, child), entity(graph, parent));
    graph
        .relations
        .iter()
        .any(|r| r.from == child.id && r.to == parent.id && r.rel == Rel::PartOf)
}

fn events(service: &ContextService) -> usize {
    service.store().events(PROJECT).unwrap().len()
}

#[test]
fn a_new_crate_and_a_new_child_are_added_parents_first() {
    let d = delta(
        &tree(&BEFORE),
        &after(
            &[],
            &[
                "crates/net/Cargo.toml",
                "crates/net/src/lib.rs",
                "crates/net/src/http/mod.rs",
                "crates/core/src/rpc/mod.rs",
            ],
        ),
    );
    assert_eq!(slugs(&d.added), ["net", "core-rpc", "net-http"]);
    assert!(d.removed.is_empty() && d.moved.is_empty(), "{d:?}");
}

#[test]
fn files_inside_existing_modules_are_no_delta() {
    let d = delta(
        &tree(&BEFORE),
        &after(
            &["crates/core/src/lib.rs"],
            &[
                "crates/core/src/main.rs",
                "crates/core/src/store/sqlite.rs",
                "src/components/Nav.tsx",
                "README.md",
            ],
        ),
    );
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn a_deleted_directory_is_removed() {
    let d = delta(&tree(&BEFORE), &after(&["crates/old/"], &[]));
    assert_eq!(slugs(&d.removed), ["old"]);
    assert!(d.added.is_empty() && d.moved.is_empty(), "{d:?}");
}

#[test]
fn a_directory_under_the_same_slug_elsewhere_has_moved() {
    let d = delta(
        &tree(&BEFORE),
        &after(
            &["crates/core/"],
            &[
                "libs/core/Cargo.toml",
                "libs/core/src/lib.rs",
                "libs/core/src/store/mod.rs",
            ],
        ),
    );
    let moved: Vec<(&str, &str)> = d
        .moved
        .iter()
        .map(|m| (m.from.as_str(), m.to.path.as_str()))
        .collect();
    assert_eq!(
        moved,
        [
            ("crates/core", "libs/core"),
            ("crates/core/src/store", "libs/core/src/store")
        ]
    );
    assert!(d.added.is_empty() && d.removed.is_empty(), "{d:?}");
}

#[test]
fn merging_a_new_crate_records_its_modules_with_paths_and_parents() {
    let (service, _db) = bootstrapped();
    let structure = delta(
        &tree(&BEFORE),
        &after(
            &[],
            &[
                "crates/net/Cargo.toml",
                "crates/net/src/lib.rs",
                "crates/net/src/http/mod.rs",
                "crates/core/src/rpc/mod.rs",
            ],
        ),
    );

    merge(&service, structure, "## Decisions\n- none\n");

    let g = graph(&service);
    let net = entity(&g, "net");
    assert_eq!(net.kind, EntityKind::Module);
    assert_eq!(net.paths, ["crates/net"]);
    assert!(net.summary.is_empty());
    assert_eq!(net.author, Author::ingester());
    assert_eq!(net.source, Source::new(SourceKind::Pr, Some(PR.into())));
    assert_eq!(entity(&g, "net-http").paths, ["crates/net/src/http"]);
    assert!(part_of(&g, "net-http", "net"));
    assert!(part_of(&g, "core-rpc", "core"));
    assert!(g.assertions.is_empty());
}

#[test]
fn merging_a_deleted_directory_archives_its_module() {
    let (service, _db) = bootstrapped();
    let structure = delta(&tree(&BEFORE), &after(&["crates/old/"], &[]));

    merge(&service, structure, "");

    let g = graph(&service);
    assert_eq!(entity(&g, "old").status, EntityStatus::Archived);
    assert_eq!(entity(&g, "core").status, EntityStatus::Active);
    let query = CompileQuery {
        entities: vec!["old".into()],
        ..Default::default()
    };
    let bundle = compile::compile(&g, &query, None);
    assert_eq!(bundle.misses, ["old"]);
    assert!(bundle.entities.is_empty());
}

#[test]
fn merging_a_move_rebases_the_anchors_and_keeps_the_curation() {
    let (service, _db) = bootstrapped();
    let core = entity(&graph(&service), "core").id.clone();
    service
        .record_entity(
            &project(),
            EntityInput {
                id: Some(core.clone()),
                slug: "core".into(),
                kind: EntityKind::Module,
                name: "Engine".into(),
                summary: "The engine.".into(),
                aliases: vec!["kernel".into()],
                paths: vec!["crates/core".into(), "crates/core/src/lib.rs".into()],
            },
            Stamp {
                author: Author::user(),
                source: Source::ui(),
                provenance: Provenance::default(),
            },
        )
        .unwrap();
    let structure = delta(
        &tree(&BEFORE),
        &after(
            &["crates/core/"],
            &[
                "libs/core/Cargo.toml",
                "libs/core/src/lib.rs",
                "libs/core/src/store/mod.rs",
            ],
        ),
    );

    merge(&service, structure, "");

    let g = graph(&service);
    let moved = g.entity(&core).unwrap();
    assert_eq!(moved.paths, ["libs/core", "libs/core/src/lib.rs"]);
    assert_eq!(moved.name, "Engine");
    assert_eq!(moved.summary, "The engine.");
    assert_eq!(moved.aliases, ["kernel"]);
    assert_eq!(moved.source, Source::new(SourceKind::Pr, Some(PR.into())));
    assert_eq!(entity(&g, "core-store").paths, ["libs/core/src/store"]);
}

/// An archive is not revised back into play: the directory's return is
/// skipped like any slug the project has had.
#[test]
fn a_directory_that_returns_leaves_its_archived_module_archived() {
    let (service, _db) = bootstrapped();
    let old = entity(&graph(&service), "old").id.clone();
    service
        .archive_entity(&project(), &old, bootstrap::stamp("base"))
        .unwrap();
    let before = events(&service);

    merge(
        &service,
        StructureDelta {
            added: after(&[], &[])
                .into_iter()
                .filter(|m| m.slug == "old")
                .collect(),
            ..Default::default()
        },
        "",
    );

    assert_eq!(events(&service), before);
    assert_eq!(
        graph(&service).entity(&old).unwrap().status,
        EntityStatus::Archived
    );
}

#[test]
fn a_pr_inside_existing_modules_writes_nothing_structural() {
    let (service, _db) = bootstrapped();
    let before = events(&service);
    let structure = delta(
        &tree(&BEFORE),
        &after(&[], &["crates/core/src/store/sqlite.rs", "docs/notes.md"]),
    );

    merge(&service, structure, "## Decisions\n- none\n");

    assert_eq!(events(&service), before);
}

#[test]
fn the_same_reference_twice_writes_nothing_the_second_time() {
    let (service, _db) = bootstrapped();
    let structure = delta(
        &tree(&BEFORE),
        &after(
            &["crates/old/"],
            &["crates/net/Cargo.toml", "crates/net/src/lib.rs"],
        ),
    );
    merge(&service, structure.clone(), "");
    let once = events(&service);
    assert!(once > 0);

    merge(&service, structure.clone(), "");
    assert_eq!(events(&service), once);
    // Without the reference check, the rules alone still write nothing.
    apply(&service, &project(), &structure, &bootstrap::stamp("again")).unwrap();
    assert_eq!(events(&service), once);
}

#[test]
fn a_decision_line_can_name_a_module_the_same_pr_added() {
    let (service, _db) = bootstrapped();
    let structure = delta(
        &tree(&BEFORE),
        &after(&[], &["crates/net/Cargo.toml", "crates/net/src/lib.rs"]),
    );

    merge(
        &service,
        structure,
        "## Decisions\n- adopted · architectural · [net] — Networking lives in its own crate.\n",
    );

    let g = graph(&service);
    let net = entity(&g, "net");
    assert!(g.assertions.iter().any(|a| a.about == [net.id.clone()]));
}

fn strings(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

fn added(path: &str) -> PrFileChange {
    PrFileChange::Added(path.into())
}

fn removed(path: &str) -> PrFileChange {
    PrFileChange::Removed(path.into())
}

fn renamed(from: &str, to: &str) -> PrFileChange {
    PrFileChange::Renamed {
        from: from.into(),
        to: to.into(),
    }
}

/// A PR of several commits, as GitHub lists its files: one change per file,
/// however many commits touched it. The tree after the merge also holds a
/// crate the base gained while the PR was open; it is not the PR's.
#[test]
fn reversing_a_multi_commit_prs_files_gives_its_delta_alone() {
    let after_files = strings(&[
        "package.json",
        "src/main.tsx",
        "src/components/App.tsx",
        "crates/core/Cargo.toml",
        "crates/core/src/lib.rs",
        "crates/core/src/store/mod.rs",
        "crates/core/src/rpc/mod.rs",
        "crates/net/Cargo.toml",
        "crates/net/src/lib.rs",
        "crates/net/src/http/mod.rs",
        "crates/base-only/Cargo.toml",
        "crates/base-only/src/lib.rs",
    ]);
    let changes = [
        added("crates/net/Cargo.toml"),
        added("crates/net/src/lib.rs"),
        added("crates/net/src/http/mod.rs"),
        added("crates/core/src/rpc/mod.rs"),
        removed("crates/old/Cargo.toml"),
        removed("crates/old/src/lib.rs"),
    ];

    let before_files = unapply(&after_files, &changes);
    let mut expected = strings(&BEFORE);
    expected.extend(strings(&[
        "crates/base-only/Cargo.toml",
        "crates/base-only/src/lib.rs",
    ]));
    expected.sort();
    assert_eq!(before_files, expected);

    let d = delta(
        &rules::modules(&before_files),
        &rules::modules(&after_files),
    );
    assert_eq!(slugs(&d.added), ["net", "core-rpc", "net-http"]);
    assert_eq!(slugs(&d.removed), ["old"]);
    assert!(d.moved.is_empty());
}

#[test]
fn a_renamed_directory_is_a_move() {
    let after_files = strings(&[
        "package.json",
        "src/main.tsx",
        "src/components/App.tsx",
        "libs/core/Cargo.toml",
        "libs/core/src/lib.rs",
        "libs/core/src/store/mod.rs",
        "crates/old/Cargo.toml",
        "crates/old/src/lib.rs",
    ]);
    let changes = [
        renamed("crates/core/Cargo.toml", "libs/core/Cargo.toml"),
        renamed("crates/core/src/lib.rs", "libs/core/src/lib.rs"),
        renamed("crates/core/src/store/mod.rs", "libs/core/src/store/mod.rs"),
    ];

    let before_files = unapply(&after_files, &changes);
    let d = delta(
        &rules::modules(&before_files),
        &rules::modules(&after_files),
    );

    let moved: Vec<(&str, &str)> = d
        .moved
        .iter()
        .map(|m| (m.from.as_str(), m.to.path.as_str()))
        .collect();
    assert_eq!(
        moved,
        [
            ("crates/core", "libs/core"),
            ("crates/core/src/store", "libs/core/src/store")
        ]
    );
    assert!(d.added.is_empty() && d.removed.is_empty(), "{d:?}");
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The merge is made on the remote, so the source repo has to fetch it. A
/// fetch that fails fails the ingest before the PR is marked; the next
/// announcement reads and lands the delta.
#[tokio::test]
async fn a_failing_read_leaves_the_pr_unseen_and_a_retry_lands_the_delta() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream");
    let write = |path: &str| {
        let file = upstream.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "").unwrap();
    };
    for path in BEFORE {
        write(path);
    }
    crate::git::init_repo(&upstream).await.unwrap();
    crate::git::commit_all(&upstream, "base").await.unwrap();
    git(
        dir.path(),
        &["clone", "-q", upstream.to_str().unwrap(), "source"],
    );
    let source = dir.path().join("source");
    write("crates/net/Cargo.toml");
    crate::git::commit_all(&upstream, "net manifest")
        .await
        .unwrap();
    write("crates/net/src/lib.rs");
    crate::git::commit_all(&upstream, "net code").await.unwrap();
    let sha = crate::git::rev_parse(&upstream, "HEAD").await.unwrap();
    let changes = [
        added("crates/net/Cargo.toml"),
        added("crates/net/src/lib.rs"),
    ];
    let (service, _db) = bootstrapped();
    let pr = MergedPr {
        reference: PR.into(),
        body: String::new(),
        branch: None,
        sha: Some(sha),
        structure: StructureDelta::default(),
    };
    let project = project();
    let ingest = |pr: MergedPr| {
        read_and_ingest(
            &service,
            &project,
            WORKSPACE,
            REPO,
            true,
            pr,
            Some((source.as_path(), &changes[..])),
        )
    };

    git(&source, &["remote", "set-url", "origin", "/nowhere"]);
    assert!(ingest(pr.clone()).await.is_err());
    assert!(!seen(&service, PROJECT, PR).unwrap());
    assert!(!graph(&service).entities.iter().any(|e| e.slug == "net"));

    git(
        &source,
        &["remote", "set-url", "origin", upstream.to_str().unwrap()],
    );
    ingest(pr).await.unwrap();
    assert!(seen(&service, PROJECT, PR).unwrap());
    let g = graph(&service);
    assert_eq!(entity(&g, "net").paths, ["crates/net"]);
}
