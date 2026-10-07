//! The deterministic base of a project's context: its modules, read from the
//! repository at one commit and recorded once. No model is involved — the
//! manifests and the directory layout say what the code is made of
//! ([`rules`]); what each part *means* is a mapping session's to write, with
//! the ordinary record ops (`instructions/context_mapping.md`).
//!
//! The log is appended to, never rebuilt: [`apply`] records only the slugs
//! the project has never had (an archived or merged one included), so a
//! second run over the same tree writes nothing and an edit, an archive or a
//! rename made since is never undone. Entities and relations only; what was
//! decided about them is not in a file tree.

pub mod rules;

use std::path::Path;

use crate::context::model::*;
use crate::context::service::{ContextService, Project};
use crate::context::Result;

/// One module candidate: a directory, the slug it is recorded under and the
/// slug of the module it is part of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub slug: String,
    pub name: String,
    /// Repo-relative directory: the entity's path anchor.
    pub path: String,
    pub parent: Option<String>,
}

/// The modules of a repository at one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skeleton {
    pub commit: String,
    /// Parents before children.
    pub modules: Vec<Module>,
}

/// What [`apply`] recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Slugs of the entities created.
    pub created: Vec<String>,
    /// `part_of` edges added.
    pub linked: usize,
}

/// Read the skeleton of the repository at `checkout`'s `HEAD`: the committed
/// tree, not the working one.
pub async fn derive(checkout: &Path) -> crate::error::Result<Skeleton> {
    let commit = crate::git::rev_parse(checkout, "HEAD").await?;
    let files = crate::git::list_files_at(checkout, &commit).await?;
    Ok(Skeleton {
        commit,
        modules: rules::modules(&files),
    })
}

/// The stamp a skeleton lands under: the ingester, from the repository at
/// `commit`.
pub fn stamp(commit: &str) -> Stamp {
    Stamp {
        author: Author::ingester(),
        source: Source::new(SourceKind::Repo, Some(commit.to_string())),
        provenance: Provenance {
            commit_sha: Some(commit.to_string()),
            ..Default::default()
        },
    }
}

/// Record every module whose slug the project has never had, as a `module`
/// entity anchored at its directory, and link each new one `part_of` its
/// parent. A slug already there is skipped, not revised. A parent that was
/// already there is linked to only when it is an active module: a feature
/// or topic that happens to share the directory's slug is not that directory.
pub fn apply(
    service: &ContextService,
    project: &Project,
    skeleton: &Skeleton,
    stamp: Stamp,
) -> Result<Applied> {
    let graph = service.graph(project)?;
    let known = |slug: &str| {
        graph
            .entities
            .iter()
            .find(|e| e.slug.eq_ignore_ascii_case(slug))
    };
    let mut ids: Vec<(String, Id)> = Vec::new();
    let mut applied = Applied::default();
    for module in &skeleton.modules {
        if known(&module.slug).is_some() {
            continue;
        }
        let id = service.record_entity(
            project,
            EntityInput {
                id: None,
                slug: module.slug.clone(),
                kind: EntityKind::Module,
                name: module.name.clone(),
                summary: String::new(),
                aliases: Vec::new(),
                paths: vec![module.path.clone()],
            },
            stamp.clone(),
        )?;
        ids.push((module.slug.clone(), id.clone()));
        applied.created.push(module.slug.clone());

        let parent = module.parent.as_deref().and_then(|slug| {
            let created = ids
                .iter()
                .find(|(s, _)| s == slug)
                .map(|(_, id)| id.clone());
            created.or_else(|| match known(slug) {
                Some(e) if e.status == EntityStatus::Active && e.kind == EntityKind::Module => {
                    Some(e.id.clone())
                }
                other => {
                    tracing::debug!(
                        project = %project.id,
                        module = %module.slug,
                        parent = slug,
                        found = ?other.map(|e| (e.kind, e.status)),
                        "context bootstrap: parent is not an active module; no part_of edge"
                    );
                    None
                }
            })
        });
        if let Some(parent) = parent {
            service.link(
                project,
                LinkChange {
                    from: id,
                    to: parent,
                    rel: Rel::PartOf,
                    add: true,
                },
                stamp.clone(),
            )?;
            applied.linked += 1;
        }
    }
    Ok(applied)
}

#[cfg(test)]
#[path = "tests/rules.rs"]
mod rules_tests;

#[cfg(test)]
#[path = "tests/apply.rs"]
mod apply_tests;
