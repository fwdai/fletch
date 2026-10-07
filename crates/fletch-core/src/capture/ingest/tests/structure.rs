//! Structural deltas: pure over file lists, then landed through the merge
//! entry ([`ingest_merged`]) over `ContextStore::temp()`, seeded by the
//! bootstrap from the tree before the merge.

use super::*;
use crate::capture::ingest::{ingest_merged, MergedPr};
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
    let bundle = compile::compile(&g, &query, None, None);
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

/// From a real repository: the merge commit against its first parent.
#[tokio::test]
async fn read_compares_a_merge_commit_with_its_first_parent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    let write = |path: &str| {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "").unwrap();
    };
    for path in BEFORE {
        write(path);
    }
    crate::git::init_repo(&root).await.unwrap();
    crate::git::commit_all(&root, "base").await.unwrap();
    std::fs::remove_dir_all(root.join("crates/old")).unwrap();
    write("crates/net/Cargo.toml");
    write("crates/net/src/lib.rs");
    crate::git::commit_all(&root, "merge").await.unwrap();
    let sha = crate::git::rev_parse(&root, "HEAD").await.unwrap();

    let d = read(&root, &sha).await.unwrap();

    assert_eq!(slugs(&d.added), ["net"]);
    assert_eq!(slugs(&d.removed), ["old"]);
    assert!(d.moved.is_empty());
}
