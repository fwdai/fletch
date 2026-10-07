//! Bootstrap through the service over `ContextStore::temp()`, from a real
//! repository committed into a temp dir.

use std::path::Path;

use super::*;
use crate::context::{compile, ContextStore};

const PROJECT: &str = "proj-ctx";

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

/// A small monorepo, committed: a root app, a crate with two code
/// directories, a nested package.
async fn repo(dir: &Path) -> std::path::PathBuf {
    let root = dir.join("repo");
    for (path, body) in [
        ("package.json", "{}"),
        ("src/main.tsx", ""),
        ("src/components/App.tsx", ""),
        ("crates/core/Cargo.toml", ""),
        ("crates/core/src/lib.rs", ""),
        ("crates/core/src/store/mod.rs", ""),
        ("crates/core/src/rpc/mod.rs", ""),
        ("mobile/package.json", "{}"),
        ("mobile/src-tauri/Cargo.toml", ""),
        ("mobile/src-tauri/src/lib.rs", ""),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    crate::git::init_repo(&root).await.unwrap();
    crate::git::commit_all(&root, "init").await.unwrap();
    root
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

#[tokio::test]
async fn an_empty_store_gets_every_module_with_paths_and_part_of_edges() {
    let dir = tempfile::tempdir().unwrap();
    let root = repo(dir.path()).await;
    let (service, _db) = service();

    let skeleton = derive(&root).await.unwrap();
    assert_eq!(skeleton.commit.len(), 40);
    let applied = apply(&service, &project(), &skeleton, stamp(&skeleton.commit)).unwrap();

    let g = graph(&service);
    let mut slugs: Vec<&str> = g.entities.iter().map(|e| e.slug.as_str()).collect();
    slugs.sort();
    assert_eq!(
        slugs,
        vec![
            "core",
            "core-rpc",
            "core-store",
            "mobile",
            "mobile-src-tauri",
            "src",
            "src-components"
        ]
    );
    assert_eq!(applied.created.len(), 7);
    for e in &g.entities {
        assert_eq!(e.kind, EntityKind::Module);
        assert_eq!(e.author, Author::ingester());
        assert_eq!(e.source.kind, SourceKind::Repo);
        assert_eq!(
            e.source.reference.as_deref(),
            Some(skeleton.commit.as_str())
        );
        assert_eq!(e.paths.len(), 1);
        assert!(root.join(&e.paths[0]).is_dir(), "{:?}", e.paths);
        assert!(e.summary.is_empty());
    }
    assert_eq!(
        entity(&g, "core-store").paths,
        vec!["crates/core/src/store"]
    );
    assert!(part_of(&g, "core-store", "core"));
    assert!(part_of(&g, "core-rpc", "core"));
    assert!(part_of(&g, "src-components", "src"));
    assert!(part_of(&g, "mobile-src-tauri", "mobile"));
    assert_eq!(applied.linked, 4);
    assert_eq!(g.relations.len(), 4);
    assert!(g.assertions.is_empty(), "bootstrap records no assertions");
}

#[tokio::test]
async fn a_second_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = repo(dir.path()).await;
    let (service, _db) = service();
    let skeleton = derive(&root).await.unwrap();
    apply(&service, &project(), &skeleton, stamp(&skeleton.commit)).unwrap();
    let before = service.store().events(PROJECT).unwrap().len();

    let again = apply(&service, &project(), &skeleton, stamp(&skeleton.commit)).unwrap();

    assert_eq!(again, Applied::default());
    assert_eq!(service.store().events(PROJECT).unwrap().len(), before);
}

/// A slug the project already has — revised, or archived — is left alone;
/// a module new since then still joins its existing parent.
#[tokio::test]
async fn existing_slugs_are_skipped_not_revised() {
    let dir = tempfile::tempdir().unwrap();
    let root = repo(dir.path()).await;
    let (service, _db) = service();
    let user = Stamp {
        author: Author::user(),
        source: Source::ui(),
        provenance: Provenance::default(),
    };
    let core = service
        .record_entity(
            &project(),
            EntityInput {
                slug: "core".into(),
                kind: EntityKind::Module,
                name: "Engine".into(),
                summary: "The engine.".into(),
                ..Default::default()
            },
            user.clone(),
        )
        .unwrap();
    let mobile = service
        .record_entity(
            &project(),
            EntityInput {
                slug: "mobile".into(),
                name: "Mobile".into(),
                ..Default::default()
            },
            user.clone(),
        )
        .unwrap();
    service.archive_entity(&project(), &mobile, user).unwrap();

    let skeleton = derive(&root).await.unwrap();
    let applied = apply(&service, &project(), &skeleton, stamp(&skeleton.commit)).unwrap();

    assert!(!applied.created.contains(&"core".to_string()));
    assert!(!applied.created.contains(&"mobile".to_string()));
    let g = graph(&service);
    let kept = g.entity(&core).unwrap();
    assert_eq!(kept.name, "Engine");
    assert_eq!(kept.summary, "The engine.");
    assert!(kept.paths.is_empty());
    assert!(part_of(&g, "core-store", "core"));
    // An archived parent is history: its new child stands alone.
    let child = entity(&g, "mobile-src-tauri");
    assert!(!g.relations.iter().any(|r| r.from == child.id));
}

/// Fletch's own repository: every crate and app directory, with no input
/// but the tree. Skipped outside a git checkout (a source tarball).
#[tokio::test]
async fn this_repository_maps_every_crate_and_app() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !root.join(".git").exists() {
        return;
    }
    let skeleton = derive(&root).await.unwrap();
    let module = |slug: &str| {
        skeleton
            .modules
            .iter()
            .find(|m| m.slug == slug)
            .unwrap_or_else(|| panic!("no `{slug}` in {:#?}", skeleton.modules))
    };
    for (slug, path, parent) in [
        ("src", "src", None),
        ("src-tauri", "src-tauri", None),
        ("fletch-core", "crates/fletch-core", None),
        ("fletch-proto", "crates/fletch-proto", None),
        ("fletch-host", "crates/fletch-host", None),
        ("mobile", "mobile", None),
        ("relay", "relay", None),
        ("mobile-src-tauri", "mobile/src-tauri", Some("mobile")),
        (
            "fletch-core-context",
            "crates/fletch-core/src/context",
            Some("fletch-core"),
        ),
        ("src-components", "src/components", Some("src")),
    ] {
        let m = module(slug);
        assert_eq!(m.path, path);
        assert_eq!(m.parent.as_deref(), parent, "{slug}");
    }
}

/// A path anchor the checkout no longer has is a warning in what an agent
/// is served for that module.
#[tokio::test]
async fn a_removed_directory_is_a_missing_path_warning() {
    let dir = tempfile::tempdir().unwrap();
    let root = repo(dir.path()).await;
    let (service, _db) = service();
    let skeleton = derive(&root).await.unwrap();
    apply(&service, &project(), &skeleton, stamp(&skeleton.commit)).unwrap();
    let query = CompileQuery {
        entities: vec!["core-store".into()],
        ..Default::default()
    };

    let fresh = compile::compile(&graph(&service), &query, None, Some(&root));
    assert!(
        !fresh.warnings.iter().any(|w| w.contains("does not exist")),
        "{:?}",
        fresh.warnings
    );

    std::fs::remove_dir_all(root.join("crates/core/src/store")).unwrap();
    let stale = compile::compile(&graph(&service), &query, None, Some(&root));
    let missing: Vec<&String> = stale
        .warnings
        .iter()
        .filter(|w| w.contains("does not exist"))
        .collect();
    assert_eq!(missing.len(), 1, "{:?}", stale.warnings);
    assert!(missing[0].contains("crates/core/src/store"));
    assert!(missing[0].contains("core-store"));
    let markdown = crate::context::render::render_markdown(&stale);
    assert!(markdown.contains("crates/core/src/store"), "{markdown}");

    // No checkout, no check.
    let unchecked = compile::compile(&graph(&service), &query, None, None);
    assert!(!unchecked
        .warnings
        .iter()
        .any(|w| w.contains("does not exist")));
}
