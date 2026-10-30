//! The user's own ssh client reaching a box: an editor's remote mode opens the
//! checkout where it lives, in the guest. Each registered box gets a `Host
//! fletch-<agent id>` entry in one file under the boxes root, which a single
//! `Include` line in `~/.ssh/config` brings in; the entry reuses the box's key,
//! host key and `fletch-vm pipe` proxy (`fletch_box::channel::render_user_entry`).
//!
//! The file is regenerated from the registry on every open, so a deleted box's
//! entry goes with it and a moved helper is picked up. A box an editor was
//! opened in is held against the idle stop until the agent is archived or the
//! app quits, as the workspace terminal holds it: the host cannot see the
//! editor's connections, so it cannot know when they end. After a restart the
//! box is stopped again, and the editor reconnects once the user opens it from
//! Fletch again, which boots it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::error::{Error, Result};

use super::registry::{BoxRegistry, SSH_SOCK};
use super::{box_holding, boxes_root, runtime_paths, Hold};

/// The generated file, next to the box directories.
const CONFIG_FILE: &str = "ssh_config";

/// The ssh `Host` alias of `agent_id`'s box.
pub fn host_alias(agent_id: &str) -> String {
    format!("fletch-{agent_id}")
}

fn config_path() -> Result<PathBuf> {
    Ok(boxes_root()?.join(CONFIG_FILE))
}

fn user_config_path() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".ssh").join("config"))
        .ok_or_else(|| Error::Other("no home directory".into()))
}

fn include_line() -> Result<String> {
    Ok(format!("Include \"{}\"", config_path()?.display()))
}

/// Whether `~/.ssh/config` already includes the generated file.
pub fn include_present() -> Result<bool> {
    let config = std::fs::read_to_string(user_config_path()?).unwrap_or_default();
    Ok(has_include(&config, &include_line()?))
}

/// Add the `Include` to `~/.ssh/config` (created if absent), at the top so
/// no `Host` block above it scopes it. The user agreed to this edit first.
/// An existing file is written in place: it may be a symlink into the user's
/// dotfiles, which a replace would sever.
pub fn add_include() -> Result<()> {
    let path = user_config_path()?;
    let line = include_line()?;
    let updated = match std::fs::read_to_string(&path) {
        Ok(current) if has_include(&current, &line) => return Ok(()),
        Ok(current) => with_include(&current, &line),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(dir) = path.parent() {
                create_private_dir(dir)?;
            }
            return write_private(&path, &with_include("", &line));
        }
        Err(e) => return Err(Error::Other(format!("read {}: {e}", path.display()))),
    };
    std::fs::write(&path, updated)
        .map_err(|e| Error::Other(format!("write {}: {e}", path.display())))
}

fn has_include(config: &str, line: &str) -> bool {
    config.lines().any(|l| l.trim() == line)
}

fn with_include(config: &str, line: &str) -> String {
    format!("{line}\n\n{config}")
}

/// A box an editor was opened in stays up: one hold per box, released with
/// the process (the registry's detach is a no-op once the box is gone).
static HOLDS: Mutex<BTreeMap<String, Hold>> = Mutex::new(BTreeMap::new());

/// Make the box that holds `tree` reachable as a `Host` alias: hold it, boot
/// it if stopped, regenerate the config. `None` for a host tree. Needs the
/// `Include` in place ([`add_include`]) for the alias to resolve.
pub fn open(tree: &Path) -> Result<Option<String>> {
    let Some(identity) = box_holding(tree) else {
        return Ok(None);
    };
    HOLDS
        .lock()
        .entry(identity.agent_id.clone())
        .or_insert_with(|| Hold::take(&identity.agent_id, &uuid::Uuid::new_v4().to_string()));
    BoxRegistry::global().channel_for(&identity.agent_id)?;
    write_config()?;
    Ok(Some(host_alias(&identity.agent_id)))
}

/// Rewrite the generated file from every registered box that has booted at
/// least once (a never-booted box has no host key to trust yet).
fn write_config() -> Result<()> {
    let helper = runtime_paths()
        .ok_or_else(|| Error::Other("box runtime not found".into()))?
        .helper;
    let mut config = String::from(
        "# Written by Fletch for the boxes on this Mac; edits are overwritten.\n\n",
    );
    for identity in BoxRegistry::global().identities() {
        let dir = identity.box_dir()?;
        let key = dir.join(fletch_box::channel::KEY_FILE);
        let known_hosts = dir.join(fletch_box::channel::KNOWN_HOSTS_FILE);
        if !key.exists() || !known_hosts.exists() {
            continue;
        }
        config.push_str(&fletch_box::channel::render_user_entry(
            &host_alias(&identity.agent_id),
            &dir.join(SSH_SOCK),
            &key,
            &known_hosts,
            &helper,
        ));
        config.push('\n');
    }
    write_private(&config_path()?, &config)
}

fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(Error::Other(format!("create {}: {e}", dir.display()))),
    }
}

/// Write `contents` to `path` with mode 0600 through a sibling temp file, so
/// a reader (ssh, mid-connect) never sees a partial file.
fn write_private(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let err = |e: std::io::Error| Error::Other(format!("write {}: {e}", path.display()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(err)?;
    file.write_all(contents.as_bytes()).map_err(err)?;
    file.sync_all().map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "Include \"/Users/me/.fletch/boxes/ssh_config\"";

    #[test]
    fn include_goes_first_and_is_added_once() {
        let config = "Host github.com\n  User git\n";
        let added = with_include(config, LINE);
        assert!(added.starts_with(LINE), "{added}");
        assert!(added.ends_with(config));
        assert!(has_include(&added, LINE));
        assert!(!has_include(config, LINE));
        // An indented or trailing-space copy counts as present.
        assert!(has_include(&format!("  {LINE} \n"), LINE));
        // A different path does not.
        assert!(!has_include(
            "Include \"/Users/me/.fletch/dev/boxes/ssh_config\"",
            LINE
        ));
    }

    #[test]
    fn private_write_replaces_atomically_with_owner_only_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        write_private(&path, "one\n").unwrap();
        write_private(&path, "two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\n");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!path.with_extension("tmp").exists());
    }
}
