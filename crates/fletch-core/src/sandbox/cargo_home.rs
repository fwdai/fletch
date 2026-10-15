//! The `CARGO_HOME` every sandboxed toolchain runs with: a Fletch-owned
//! overlay under the toolchain cache root, with the user's cargo config and
//! registry credentials linked in from their real home.
//!
//! Cargo keeps its registry and git caches under `CARGO_HOME` and has no
//! cache-only variable to move them, so it can't take the plain env redirect
//! [`super::policy::TOOLCHAIN_CACHE_VARS`] gives every other toolchain. Two
//! other routes were tried or proposed and are rejected:
//!
//! - **Redirecting `CARGO_HOME` bare** (e8b54deb, reverted in c86d3235) hid
//!   `~/.cargo/config.toml` and `credentials.toml`: a user's global cargo
//!   config and private-registry tokens silently vanished.
//! - **Granting cache islands under the real `~/.cargo`** (`registry/`,
//!   `git/` plus the sibling lock files), which c86d3235 proposed instead, is
//!   a host code-execution hole. `registry/src` and `git/checkouts` hold the
//!   unpacked sources — `build.rs`, proc macros — that cargo compiles and
//!   *runs*, and cargo trusts an already-extracted dir on sight (its
//!   `.cargo-ok` marker; nothing re-verifies the files against the index).
//!   An agent that edits one poisons the user's **next host build**, outside
//!   any sandbox — the class invariants 3 and 4 in [`super::policy`] close.
//!
//! The overlay keeps both properties. Cargo sees its config through links into
//! the real home (reads are unrestricted; a write through a link resolves to
//! the ungranted real file and is denied), while everything it *writes* lands
//! in a tree only sandboxed processes ever build from: a crate tampered with
//! here is compiled only under the agent or Run profile, never by the user's
//! host cargo. The cost is one re-download of a project's crates into the
//! overlay; after that every agent and the Run panel share it, like every
//! other redirected cache. The rustup `cargo` proxy is unaffected — it locates
//! toolchains through `RUSTUP_HOME`, which stays the user's.
//!
//! What an agent can do to the overlay is bounded the way the checkout is. It
//! may replace the `config.toml` link with a file of its own, which steers only
//! sandboxed cargo runs — the Run panel included, but an agent already steers
//! those through the project's own `.cargo/config.toml` or a `build.rs` (the
//! documented Run trade-off, `seatbelt::RUN_TOOLCHAIN_DIRS`), and the next
//! launch puts the link back. `cargo install` lands binaries in the overlay's
//! `bin/`, which is never on the user's PATH — invariant 1 is exactly why a
//! redirected home is safe to grant where the real `~/.cargo` is not.

use std::path::{Path, PathBuf};

use crate::agent::credential_file::{Entry, PrivateDir};

/// Subdir of the toolchain cache root serving as the sandboxed `CARGO_HOME`.
pub const SUBDIR: &str = "cargo";

/// What cargo reads from its home that is *not* cache, linked in from the
/// user's real home: the global config (current and legacy name) and the
/// registry credentials (likewise).
const SHARED_FILES: &[&str] = &["config.toml", "config", "credentials.toml", "credentials"];

/// The overlay under `cache_root` — the value of `CARGO_HOME` a child gets.
pub fn overlay(cache_root: &Path) -> PathBuf {
    cache_root.join(SUBDIR)
}

/// The user's own cargo home, where the shared files come from: `CARGO_HOME`
/// as a launched child would see it (the login shell's over the app's), else
/// `~/.cargo`.
pub fn user_home(home: &Path) -> PathBuf {
    user_home_from(crate::bin_resolve::effective_env_var("CARGO_HOME"), home)
}

/// Pure core of [`user_home`] — the env seam split the other resolvers in
/// [`super::policy`] use, so it tests hermetically. Two values fall back to
/// `~/.cargo`:
///
/// - **A relative path.** Cargo resolves a relative `CARGO_HOME` against each
///   process's working directory, so there is no one config to link; and a
///   relative link target would resolve from inside the overlay, never where
///   `exists()` looked.
/// - **A path inside the toolchain cache root.** A nested Fletch launched from
///   the Run panel inherits the overlay as its `CARGO_HOME` (the Run env is
///   its process env). Taken at face value, its own `prepare` would relink the
///   overlay's config to *itself* — a dangling loop that hides the config from
///   every instance sharing the overlay. The nested instance's real home is the
///   same user's `~/.cargo`.
fn user_home_from(value: Option<std::ffi::OsString>, home: &Path) -> PathBuf {
    let default = home.join(".cargo");
    let Some(v) = value.filter(|v| !v.is_empty()) else {
        return default;
    };
    let dir = PathBuf::from(v);
    let cache_root = super::policy::toolchain_cache_root(home);
    if dir.is_relative()
        || dir.starts_with(&cache_root)
        || super::policy::resolve_existing_prefix(&dir)
            .starts_with(super::policy::resolve_existing_prefix(&cache_root))
    {
        return default;
    }
    dir
}

/// Create (or repair) the overlay under `cache_root`, linking each shared file
/// that exists in `source` and dropping a link whose source is gone. The
/// overlay is agent-writable, so every step goes through a no-follow handle
/// ([`PrivateDir`]): an overlay the agent swapped for a link is replaced by a
/// real dir, and a file it left where a link belongs is replaced by the link —
/// never the other way round, and never touching anything in `source`.
/// `cache_root` itself must already exist and be the host's own.
///
/// The overlay is shared, so two launches can prepare it at once. Each step
/// is a single syscall and idempotent, and a link that another launch created
/// between our look and our `symlink` is accepted when it is the one we
/// wanted — the only loser in that race would otherwise be a launch that
/// found the overlay already correct.
pub fn prepare(cache_root: &Path, source: &Path) -> std::io::Result<()> {
    let dir = PrivateDir::open(cache_root)?.subdir(SUBDIR)?;
    for item in SHARED_FILES {
        let target = source.join(item);
        let wanted = target.exists();
        match dir.entry(item)? {
            Entry::Link(at) if wanted && at == target => continue,
            Entry::Missing => {}
            _ => dir.remove(item)?,
        }
        if wanted {
            match dir.symlink(&target, item) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if dir.entry(item)? != Entry::Link(target) {
                        return Err(e);
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

/// The env pair pointing cargo at the overlay.
pub fn env(cache_root: &Path) -> (String, String) {
    (
        "CARGO_HOME".to_string(),
        overlay(cache_root).to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let cache_root = td.path().join("toolchain-cache");
        let source = td.path().join(".cargo");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        (td, cache_root, source)
    }

    #[test]
    fn user_home_honors_an_absolute_cargo_home_and_falls_back_otherwise() {
        let home = Path::new("/Users/u");
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(user_home_from(None, home), home.join(".cargo"));
        assert_eq!(user_home_from(os(""), home), home.join(".cargo"));
        assert_eq!(
            user_home_from(os("/opt/cargo"), home),
            PathBuf::from("/opt/cargo")
        );
        // Relative: cargo resolves it per working directory, nothing to link.
        assert_eq!(
            user_home_from(os("vendor/cargo"), home),
            home.join(".cargo")
        );
        // The overlay itself, as a nested Fletch inherits it from the Run env:
        // linking the overlay's config to itself would hide it from everyone.
        let inherited = overlay(&super::super::policy::toolchain_cache_root(home));
        assert_eq!(
            user_home_from(Some(inherited.into_os_string()), home),
            home.join(".cargo")
        );
    }

    #[test]
    fn env_points_inside_the_granted_root() {
        let root = Path::new("/Users/u/.fletch/toolchain-cache");
        let (var, value) = env(root);
        assert_eq!(var, "CARGO_HOME");
        let p = PathBuf::from(value);
        assert!(p.starts_with(root) && p != root);
    }

    #[test]
    fn prepare_links_only_the_shared_files_that_exist() {
        let (_td, cache_root, source) = roots();
        std::fs::write(source.join("config.toml"), "[build]\njobs = 2\n").unwrap();
        std::fs::write(
            source.join("credentials.toml"),
            "[registry]\ntoken = \"t\"\n",
        )
        .unwrap();

        prepare(&cache_root, &source).unwrap();

        let overlay = overlay(&cache_root);
        assert!(overlay.is_dir());
        for item in ["config.toml", "credentials.toml"] {
            let link = overlay.join(item);
            assert_eq!(
                std::fs::read_link(&link).unwrap(),
                source.join(item),
                "{item}"
            );
        }
        for item in ["config", "credentials"] {
            assert!(
                overlay.join(item).symlink_metadata().is_err(),
                "{item} has no source, so no link"
            );
        }
        // Cargo reads the config through the link.
        assert_eq!(
            std::fs::read_to_string(overlay.join("config.toml")).unwrap(),
            "[build]\njobs = 2\n"
        );
    }

    #[test]
    fn prepare_repairs_what_the_agent_changed_and_drops_dangling_links() {
        let (_td, cache_root, source) = roots();
        std::fs::write(source.join("config.toml"), "a").unwrap();
        std::fs::write(source.join("credentials.toml"), "b").unwrap();
        prepare(&cache_root, &source).unwrap();
        let overlay = overlay(&cache_root);

        // An agent swapped the config link for a file of its own, and the user
        // deleted their credentials since the last launch.
        std::fs::remove_file(overlay.join("config.toml")).unwrap();
        std::fs::write(overlay.join("config.toml"), "[alias]\nb = \"evil\"\n").unwrap();
        std::fs::remove_file(source.join("credentials.toml")).unwrap();

        prepare(&cache_root, &source).unwrap();

        assert_eq!(
            std::fs::read_link(overlay.join("config.toml")).unwrap(),
            source.join("config.toml")
        );
        assert!(
            overlay.join("credentials.toml").symlink_metadata().is_err(),
            "a link whose source is gone is removed, not left dangling"
        );
        // The user's own file is untouched throughout.
        assert_eq!(
            std::fs::read_to_string(source.join("config.toml")).unwrap(),
            "a"
        );
    }

    #[test]
    fn prepare_survives_concurrent_launches_on_a_fresh_overlay() {
        let (_td, cache_root, source) = roots();
        for item in SHARED_FILES {
            std::fs::write(source.join(item), item).unwrap();
        }
        // Every launch prepares the same shared overlay; on first use they all
        // see the links missing and race to create them. None may fail.
        let results: Vec<std::io::Result<()>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| s.spawn(|| prepare(&cache_root, &source)))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for r in results {
            r.unwrap();
        }
        let overlay = overlay(&cache_root);
        for item in SHARED_FILES {
            assert_eq!(
                std::fs::read_link(overlay.join(item)).unwrap(),
                source.join(item)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn prepare_never_follows_an_overlay_the_agent_swapped_for_a_link() {
        let (_td, cache_root, source) = roots();
        std::fs::write(source.join("config.toml"), "a").unwrap();
        // The agent replaced the whole overlay with a link to the user's real
        // cargo home. A host that followed it would unlink the user's
        // `config.toml` and plant a self-referential link in its place.
        std::os::unix::fs::symlink(&source, overlay(&cache_root)).unwrap();

        prepare(&cache_root, &source).unwrap();

        let overlay = overlay(&cache_root);
        assert!(
            overlay.symlink_metadata().unwrap().is_dir(),
            "the overlay is a real directory again"
        );
        assert_eq!(
            std::fs::read_link(overlay.join("config.toml")).unwrap(),
            source.join("config.toml")
        );
        assert!(
            source
                .join("config.toml")
                .symlink_metadata()
                .unwrap()
                .is_file(),
            "the user's config is still their regular file"
        );
        assert_eq!(
            std::fs::read_to_string(source.join("config.toml")).unwrap(),
            "a"
        );
    }
}
