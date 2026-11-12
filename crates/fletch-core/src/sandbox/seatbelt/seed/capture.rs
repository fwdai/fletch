//! What a seed keeps of a workspace on this Mac. The box's rules
//! (`boxes/seed.sh`), and one more:
//!
//! - Kept: in each checkout, the directories git ignores as a whole and that
//!   hold at least [`WORTH_INHERITING_KIB`] (`node_modules`, `target`, build
//!   output), where they were.
//! - Not kept: Fletch's own index (`.codegraph`); a directory tied to its
//!   path, which holds an executable whose `#!` line names the workspace (a
//!   virtualenv's scripts); and then whatever is at the paths the project
//!   excludes, a kept directory, inside one or above one.
//!
//! The checkouts are the agent's to write, so no link in them is followed and
//! no path git lists is trusted to stay inside its checkout.

use std::ffi::OsStr;
use std::fs::Metadata;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use fletch_box::disks;
use nix::libc;

use crate::error::{Error, Result};
use crate::sandbox::seed::WORTH_INHERITING_KIB;

/// The most of a `#!` line macOS reads.
const SHEBANG_BYTES: u64 = 512;

/// Clones into `tree`, under each checkout's name, what a seed keeps of the
/// checkouts in `workspace`, and answers the KiB it kept. `tree` is canonical.
pub(super) async fn keep(workspace: &Path, tree: &Path, excluded: &[String]) -> Result<u64> {
    let workspace = workspace.canonicalize()?;
    let mut listed = Vec::new();
    for checkout in checkouts(&workspace).await? {
        let ignored = ignored_dirs(&checkout).await?;
        listed.push((checkout, ignored));
    }
    let (tree, excluded) = (tree.to_path_buf(), excluded.to_vec());
    let kept = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&tree)?;
        for (checkout, ignored) in &listed {
            keep_checkout(&workspace, checkout, ignored, &tree, &excluded)?;
        }
        held_kib(&tree)
    });
    let kept_kib = kept
        .await
        .map_err(|e| Error::Other(format!("keeping a workspace's directories: {e}")))??;
    Ok(kept_kib)
}

/// The checkouts in `workspace`: the directories in it that are a git top
/// level, which leaves Fletch's own out.
async fn checkouts(workspace: &Path) -> io::Result<Vec<PathBuf>> {
    let dirs: Vec<PathBuf> = std::fs::read_dir(workspace)?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    let mut checkouts = Vec::new();
    for dir in dirs {
        if is_git_top_level(&dir).await {
            checkouts.push(dir);
        }
    }
    Ok(checkouts)
}

async fn is_git_top_level(dir: &Path) -> bool {
    let top_level = crate::git::git_output(dir, &["rev-parse", "--show-toplevel"]).await;
    top_level.is_ok_and(|out| {
        out.status.success() && out.stdout.strip_suffix(b"\n") == Some(dir.as_os_str().as_bytes())
    })
}

/// The directories git ignores as a whole in `checkout`, relative to it.
async fn ignored_dirs(checkout: &Path) -> Result<Vec<PathBuf>> {
    let listed = crate::git::run_git(
        checkout,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
        "list what git ignores",
    )
    .await?;
    Ok(listed
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|entry| entry.strip_suffix(b"/"))
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(OsStr::from_bytes(dir)))
        .collect())
}

/// Clones what a seed keeps of `checkout`, of the directories git ignores
/// there, to `tree/<checkout's name>`, then removes the `excluded` paths.
fn keep_checkout(
    workspace: &Path,
    checkout: &Path,
    ignored: &[PathBuf],
    tree: &Path,
    excluded: &[String],
) -> io::Result<()> {
    let Some(name) = checkout.file_name() else {
        return Ok(());
    };
    let kept = tree.join(name);
    for rel in ignored {
        let Some(dir) = dir_beneath(checkout, rel) else {
            continue;
        };
        if is_kept(&dir, workspace)? {
            disks::clone_tree_sharing_blocks(&dir, &kept.join(rel))?;
        }
    }
    for path in excluded {
        remove_beneath(&kept, Path::new(path))?;
    }
    Ok(())
}

/// Whether a seed keeps `dir`, a directory git ignores in a checkout of
/// `workspace`.
fn is_kept(dir: &Path, workspace: &Path) -> io::Result<bool> {
    Ok(
        dir.file_name() != Some(OsStr::new(crate::codegraph::INDEX_DIR))
            && held_kib(dir)? >= WORTH_INHERITING_KIB
            && !tied_to_path(dir, workspace)?,
    )
}

/// The KiB what is under `dir` takes on disk.
fn held_kib(dir: &Path) -> io::Result<u64> {
    let mut blocks = 0;
    walk(dir, &mut |_, entry| {
        blocks += entry.blocks();
        Ok(())
    })?;
    Ok(blocks / 2)
}

/// Whether `dir` holds an executable whose `#!` line names a path in
/// `workspace`, which works only where it was built.
fn tied_to_path(dir: &Path, workspace: &Path) -> io::Result<bool> {
    let mut in_workspace = workspace.as_os_str().as_bytes().to_vec();
    in_workspace.push(b'/');
    let mut tied = false;
    walk(dir, &mut |path, entry| {
        if !tied && entry.is_file() && entry.permissions().mode() & 0o111 != 0 {
            tied = shebang_names(path, &in_workspace)?;
        }
        Ok(())
    })?;
    Ok(tied)
}

/// Whether the regular file at `path` starts with a `#!` line holding
/// `named`.
fn shebang_names(path: &Path, named: &[u8]) -> io::Result<bool> {
    // `O_NONBLOCK`: a FIFO put in the file's place opens at once.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Ok(false);
    }
    let mut head = Vec::new();
    file.take(SHEBANG_BYTES).read_to_end(&mut head)?;
    let line = head.split(|byte| *byte == b'\n').next().unwrap_or_default();
    Ok(line.starts_with(b"#!") && line.windows(named.len()).any(|bytes| bytes == named))
}

/// Calls `visit` with every entry under `dir` and what it is, never
/// following a link.
fn walk(dir: &Path, visit: &mut impl FnMut(&Path, &Metadata) -> io::Result<()>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let entry = std::fs::symlink_metadata(&path)?;
        visit(&path, &entry)?;
        if entry.is_dir() {
            walk(&path, visit)?;
        }
    }
    Ok(())
}

/// `rel` in `root`, when it is a directory reached through directories only:
/// plain names, and no link on the way or at its end.
fn dir_beneath(root: &Path, rel: &Path) -> Option<PathBuf> {
    let mut dir = root.to_path_buf();
    for part in rel.components() {
        let Component::Normal(name) = part else {
            return None;
        };
        dir.push(name);
        if !std::fs::symlink_metadata(&dir).is_ok_and(|entry| entry.is_dir()) {
            return None;
        }
    }
    Some(dir)
}

/// Removes what is at `rel` in `root`, when it is reached through directories
/// only.
fn remove_beneath(root: &Path, rel: &Path) -> io::Result<()> {
    let parent = rel.parent().and_then(|parent| dir_beneath(root, parent));
    match (parent, rel.file_name()) {
        (Some(parent), Some(name)) => super::remove(&parent.join(name)),
        _ => Ok(()),
    }
}
