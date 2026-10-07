//! The structural side of a merge: which modules it added, removed or moved,
//! by the bootstrap's own rules ([`rules::modules`]) over the tree before the
//! merge and the tree after it. The log is never re-synced with the code, so
//! a merged PR is where a change of structure enters the context.
//!
//! "After" is the merge commit's tree; "before" is that tree with the PR's
//! own file changes reversed ([`unapply`]), so it is the PR alone whether it
//! landed as a merge commit, a squash or a rebase of many commits. A module
//! is its slug; one whose slug stays but whose directory changes has moved.
//! Only the project's primary repo is read, the one the bootstrap mapped.

use std::collections::BTreeSet;
use std::path::Path;

use crate::capture::bootstrap::{self, rules, Module};
use crate::context::model::*;
use crate::context::service::{ContextService, Project};
use crate::context::Result;
use crate::github::PrFileChange;

/// What a merge did to the project's modules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StructureDelta {
    /// Parents before children.
    pub added: Vec<Module>,
    pub removed: Vec<Module>,
    pub moved: Vec<Moved>,
}

/// A module found under a new directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    /// The directory it was at.
    pub from: String,
    pub to: Module,
}

impl StructureDelta {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.moved.is_empty()
    }
}

/// The delta from `before` to `after`, matched by slug.
pub fn delta(before: &[Module], after: &[Module]) -> StructureDelta {
    let find = |modules: &[Module], slug: &str| modules.iter().find(|m| m.slug == slug).cloned();
    let mut out = StructureDelta::default();
    for module in after {
        match find(before, &module.slug) {
            None => out.added.push(module.clone()),
            Some(was) if was.path != module.path => out.moved.push(Moved {
                from: was.path,
                to: module.clone(),
            }),
            Some(_) => {}
        }
    }
    out.removed = before
        .iter()
        .filter(|m| find(after, &m.slug).is_none())
        .cloned()
        .collect();
    out
}

/// The files of the tree before a PR, from the files after it and the
/// PR's own changes: what it added is taken out, what it removed is put back,
/// a rename goes back to its old path.
pub fn unapply(after: &[String], changes: &[PrFileChange]) -> Vec<String> {
    let mut files: BTreeSet<String> = after.iter().cloned().collect();
    for change in changes {
        match change {
            PrFileChange::Added(path) => {
                files.remove(path);
            }
            PrFileChange::Removed(path) => {
                files.insert(path.clone());
            }
            PrFileChange::Renamed { from, to } => {
                files.remove(to);
                files.insert(from.clone());
            }
        }
    }
    files.into_iter().collect()
}

/// Read the delta a PR's merge commit made, from `source_repo`'s object store
/// (fetching the commit from `origin` first when it is not local) and the
/// files the PR changed.
pub async fn read(
    source_repo: &Path,
    merge_sha: &str,
    changes: &[PrFileChange],
) -> crate::error::Result<StructureDelta> {
    crate::git::fetch_commit(source_repo, merge_sha).await?;
    let after = crate::git::list_files_at(source_repo, merge_sha).await?;
    let before = unapply(&after, changes);
    Ok(delta(&rules::modules(&before), &rules::modules(&after)))
}

/// Land a delta through the service under `stamp`. Every rule is against
/// what the project has now, so a delta applied twice writes nothing the
/// second time:
///
/// - an added module is recorded as the bootstrap records one
///   ([`bootstrap::apply`]): a slug the project has ever had is skipped, so a
///   directory that comes back after its module was archived leaves that
///   module archived (an archive is not revised back into play);
/// - a removed module's active `module` entity is archived;
/// - a moved one's active `module` entity is revised with each path anchor at
///   or under the old directory rebased onto the new one, every other field
///   kept.
///
/// Only entities of kind `module` are touched: a slug a person gave to
/// something else is not a directory.
pub fn apply(
    service: &ContextService,
    project: &Project,
    delta: &StructureDelta,
    stamp: &Stamp,
) -> Result<()> {
    if delta.is_empty() {
        return Ok(());
    }
    let graph = service.graph(project)?;
    let module = |slug: &str| {
        graph.entities.iter().find(|e| {
            e.slug.eq_ignore_ascii_case(slug)
                && e.kind == EntityKind::Module
                && e.status == EntityStatus::Active
        })
    };
    for removed in &delta.removed {
        if let Some(entity) = module(&removed.slug) {
            service.archive_entity(project, &entity.id, stamp.clone())?;
        }
    }
    for Moved { from, to } in &delta.moved {
        let Some(entity) = module(&to.slug) else {
            continue;
        };
        let paths: Vec<String> = entity
            .paths
            .iter()
            .map(|p| rebase(p, from, &to.path))
            .collect();
        if paths == entity.paths {
            continue;
        }
        service.record_entity(
            project,
            EntityInput {
                id: Some(entity.id.clone()),
                slug: entity.slug.clone(),
                kind: entity.kind,
                name: entity.name.clone(),
                summary: entity.summary.clone(),
                aliases: entity.aliases.clone(),
                paths,
            },
            stamp.clone(),
        )?;
    }
    bootstrap::apply(service, project, &delta.added, stamp.clone())?;
    Ok(())
}

/// `path` with its `from` prefix (a whole directory) replaced by `to`.
fn rebase(path: &str, from: &str, to: &str) -> String {
    if path == from {
        return to.to_string();
    }
    match path
        .strip_prefix(from)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        Some(rest) => format!("{to}/{rest}"),
        None => path.to_string(),
    }
}

#[cfg(test)]
#[path = "tests/structure.rs"]
mod tests;
