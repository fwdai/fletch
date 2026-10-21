//! The `CARGO_HOME` every sandboxed toolchain runs with: a Fletch-owned
//! overlay under the toolchain cache root, with the user's registry
//! credentials linked in from their real home and their global config reached
//! the way cargo already reaches it, by walking up from the working directory.
//!
//! Cargo keeps its registry and git caches under `CARGO_HOME` and has no
//! cache-only variable to move them, so it can't take the plain env redirect
//! [`super::toolchain_cache`] gives every other toolchain. Two
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
//! The overlay keeps both properties. Everything cargo *writes* lands in a tree
//! only sandboxed processes ever build from: a crate tampered with here is
//! compiled only under the agent or Run profile, never by the user's host
//! cargo. The cost is one re-download of a project's crates per project and
//! per platform (the overlay is keyed by both, see [`overlay`]); after that
//! every agent and the Run panel of that project on that platform share it,
//! like every other redirected cache. The rustup `cargo` proxy is unaffected —
//! it locates toolchains through `RUSTUP_HOME`, which stays the user's.
//!
//! What cargo *reads* from its home splits in two:
//!
//! - **`credentials.toml`** (and the legacy `credentials`) is read from
//!   `CARGO_HOME` only, holds tokens and no paths, so it is linked in. Reads
//!   are unrestricted; a write through the link resolves to the ungranted real
//!   file and is denied, so a token rotation fails closed rather than landing
//!   in the overlay. In a Linux container the host `~/.cargo` is not mounted,
//!   so the links dangle and cargo treats the credentials as absent (no image
//!   ships cargo today).
//! - **`config.toml`** is deliberately **not** linked. Cargo resolves every
//!   relative value in a config file — `include`, `paths`, `[patch]` paths,
//!   `target-dir`, runners, `[env] relative = true` — against the parent of
//!   the directory the file was *found* in, without resolving symlinks; a link
//!   in the overlay would rebase `~/.cargo/config.toml` from `~` onto the cache
//!   root (measured: `target-dir = "tgt"` lands in the cache root).
//!   Cargo already finds the real file by its other route: it walks every
//!   ancestor of the working directory for a `.cargo/config.toml`, and a
//!   project anywhere under `~` reaches `~/.cargo/config.toml` that way with
//!   the exact documented semantics (and loads it only once; a link would add
//!   a second, wrongly-based copy).
//!
//! When that walk would **not** reach an existing global config — a checkout
//! outside `~` (`FLETCH_WORKSPACES_ROOT`), a Run target outside `~`, or a
//! non-default `CARGO_HOME` — cargo is **not redirected** ([`config_reachable`]).
//! It keeps its real home, so its config is exactly what it is on the host;
//! under the agent profile it then can't fetch a crate it hasn't got, which is
//! what every layout had before the overlay existed, and the Run profile
//! grants the real home anyway. Losing the global config silently is the
//! regression that reverted the bare redirect, and it is never traded for a
//! fetch. The route that would serve those layouts too — a host-written
//! overlay `config.toml` holding an absolute `include` of the real file, which
//! keeps the included file's own definition path (measured on cargo 1.97) —
//! is not taken yet: `include` is stable only since cargo 1.94 (March 2026),
//! and an older cargo ignores the key without a word (measured on 1.90), which
//! is that same silent loss; and for a project under `~` it would load the
//! file a second time behind the walk and double every array value (aliases,
//! `rustflags`, `paths`). Revisit when 1.94 is the oldest cargo Fletch serves.
//!
//! What an agent can do to the overlay is bounded the way the checkout is. It
//! may drop a `config.toml` of its own in there, which steers only sandboxed
//! cargo runs — the Run panel included, but an agent already steers those
//! through the project's own `.cargo/config.toml` or a `build.rs` (the
//! documented Run trade-off, `seatbelt::RUN_TOOLCHAIN_DIRS`), and the next
//! launch removes it. `cargo install` lands binaries in the overlay's `bin/`,
//! which is never on the user's PATH — invariant 1 is exactly why a redirected
//! home is safe to grant where the real `~/.cargo` is not.

use std::path::{Path, PathBuf};

use super::toolchain_cache::BoxPlatform;
use crate::agent::credential_file::{Entry, PrivateDir};

/// Subdir of the toolchain cache root serving as the sandboxed `CARGO_HOME`.
pub const SUBDIR: &str = "cargo";

/// What is linked in from the user's real home: the registry credentials,
/// current and legacy name. Never the config — see the module doc.
const SHARED_FILES: &[&str] = &["credentials.toml", "credentials"];

/// What the overlay must never carry: a config file. One the agent planted
/// would steer every sandboxed cargo run that shares the overlay, and one
/// Fletch linked would be resolved against the wrong base (module doc).
const CONFIG_FILES: &[&str] = &["config.toml", "config"];

/// The env var carrying the user's real cargo home to a redirected child, so
/// a nested Fletch — which inherits the overlay as its own `CARGO_HOME` —
/// still knows where the credentials come from ([`user_home_from`]).
pub const USER_HOME_ENV: &str = "FLETCH_CARGO_USER_HOME";

/// The overlay under `cache_root` — the value of `CARGO_HOME` a child gets.
///
/// Keyed by the child's [`BoxPlatform`], like the Playwright and npm rows of
/// the cache table: `cargo install` puts its binaries and install records
/// (`bin/`, `.crates.toml`) in the home, so a macOS agent and a Linux
/// container of one project would otherwise overwrite each other's tools. The
/// registry and git caches beside them are platform-neutral and could be
/// shared, but cargo has no variable to split one from the other, and a
/// second download of a project's crates per platform is the cheaper price.
pub fn overlay(cache_root: &Path, platform: BoxPlatform) -> PathBuf {
    cache_root.join(SUBDIR).join(platform.key())
}

/// The user's own cargo home, where the shared files come from: `CARGO_HOME`
/// as a launched child would see it (the login shell's over the app's), else
/// `~/.cargo`.
pub fn user_home(home: &Path) -> PathBuf {
    user_home_from(
        crate::bin_resolve::effective_env_var("CARGO_HOME"),
        std::env::var_os(USER_HOME_ENV),
        home,
    )
}

/// Pure core of [`user_home`] — the env seam split the other resolvers in
/// [`super::policy`] use, so it tests hermetically.
///
/// `propagated` ([`USER_HOME_ENV`]) being set at all is the signal that
/// `cargo_home` is an *inherited* overlay: Fletch only ever sets the two
/// together, and a nested Fletch launched from the Run panel gets both in its
/// process env. Taken at face value, that `CARGO_HOME` would have the nested
/// instance's own `prepare` relink the overlay's credentials to *themselves*,
/// a dangling loop that hides them from every launch sharing the overlay. So
/// the propagated value wins, which also keeps a parent's custom `/opt/cargo`
/// across the nesting.
///
/// A blank or relative value falls back to `~/.cargo`: cargo resolves a
/// relative `CARGO_HOME` against each process's working directory, so there is
/// no one credentials file to link, and a relative link target would resolve
/// from inside the overlay, never where `exists()` looked.
fn user_home_from(
    cargo_home: Option<std::ffi::OsString>,
    propagated: Option<std::ffi::OsString>,
    home: &Path,
) -> PathBuf {
    propagated
        .or(cargo_home)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".cargo"))
}

/// Whether cargo, run in `cwd` with the overlay as its home, still sees the
/// user's global config: either `user_home` holds none, or cargo's ancestor
/// walk reaches it — the home is a `.cargo` dir and `cwd` lies under its
/// parent (the walk looks for `<ancestor>/.cargo/config.toml`, so a home
/// named anything else is never found that way). When this is false the
/// launch keeps cargo's real home rather than lose the config (module doc).
pub fn config_reachable(user_home: &Path, cwd: &Path) -> bool {
    if !CONFIG_FILES.iter().any(|f| user_home.join(f).exists()) {
        return true;
    }
    if user_home.file_name() != Some(std::ffi::OsStr::new(".cargo")) {
        return false;
    }
    let Some(parent) = user_home.parent() else {
        return false;
    };
    let cwd = super::policy::resolve_existing_prefix(cwd);
    cwd.starts_with(parent) || cwd.starts_with(super::policy::resolve_existing_prefix(parent))
}

/// Create (or repair) the overlay under `cache_root`, linking each shared file
/// that exists in `source`, dropping a link whose source is gone, and removing
/// any config file. The overlay is agent-writable, so every step goes through
/// a no-follow handle ([`PrivateDir`]): an overlay the agent swapped for a link
/// is replaced by a real dir, and a file it left where a link belongs is
/// replaced by the link — never the other way round, and never touching
/// anything in `source`. `cache_root` itself must already exist and be the
/// host's own.
///
/// The overlay is shared, so two launches can prepare it at once. Each step
/// is a single syscall and idempotent, and a link that another launch created
/// between our look and our `symlink` is accepted when it is the one we
/// wanted — the only loser in that race would otherwise be a launch that
/// found the overlay already correct.
pub fn prepare(cache_root: &Path, source: &Path, platform: BoxPlatform) -> std::io::Result<()> {
    let dir = PrivateDir::open(cache_root)?
        .subdir(SUBDIR)?
        .subdir(&platform.key())?;
    for item in CONFIG_FILES {
        dir.remove(item)?;
    }
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

/// The env pointing cargo at the overlay, and telling a nested Fletch which
/// real home it was prepared from.
pub fn env(cache_root: &Path, user_home: &Path, platform: BoxPlatform) -> [(String, String); 2] {
    [
        (
            "CARGO_HOME".to_string(),
            overlay(cache_root, platform).to_string_lossy().into_owned(),
        ),
        (
            USER_HOME_ENV.to_string(),
            user_home.to_string_lossy().into_owned(),
        ),
    ]
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
        assert_eq!(user_home_from(None, None, home), home.join(".cargo"));
        assert_eq!(user_home_from(os(""), None, home), home.join(".cargo"));
        assert_eq!(
            user_home_from(os("/opt/cargo"), None, home),
            PathBuf::from("/opt/cargo")
        );
        // Relative: cargo resolves it per working directory, nothing to link.
        assert_eq!(
            user_home_from(os("vendor/cargo"), None, home),
            home.join(".cargo")
        );
    }

    /// A nested Fletch inherits the overlay as its `CARGO_HOME` together with
    /// [`USER_HOME_ENV`]. Linking the overlay's credentials to themselves would
    /// hide them from everyone, and assuming `~/.cargo` would swap a parent's
    /// custom home for the default, so the propagated home wins whatever
    /// `CARGO_HOME` says.
    #[test]
    fn user_home_of_a_nested_fletch_is_the_one_its_parent_passed_down() {
        let home = Path::new("/Users/u");
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        let inherited = || {
            let root = Path::new("/Users/u/.fletch/toolchain-cache/p1");
            Some(overlay(root, BoxPlatform::host()).into_os_string())
        };
        assert_eq!(
            user_home_from(inherited(), os("/opt/cargo"), home),
            PathBuf::from("/opt/cargo")
        );
        // A relative or blank hand-me-down is no better than none, and never
        // lets the inherited overlay through.
        assert_eq!(
            user_home_from(inherited(), os("cargo"), home),
            home.join(".cargo")
        );
        assert_eq!(
            user_home_from(inherited(), os(""), home),
            home.join(".cargo")
        );
    }

    #[test]
    fn env_points_inside_the_granted_root_and_passes_the_user_home_down() {
        let root = Path::new("/Users/u/.fletch/toolchain-cache/p1");
        let user = Path::new("/opt/cargo");
        let [(var, value), (passed_var, passed)] = env(root, user, BoxPlatform::host());
        assert_eq!(var, "CARGO_HOME");
        let p = PathBuf::from(value);
        assert!(p.starts_with(root) && p != root);
        assert_eq!(passed_var, USER_HOME_ENV);
        assert_eq!(PathBuf::from(passed), user);
    }

    /// The redirect happens only where cargo's ancestor walk still reaches an
    /// existing global config: a `.cargo` home whose parent is above `cwd`.
    /// Anything else keeps the real home rather than lose the config.
    #[test]
    fn config_is_reachable_only_from_under_the_home_dir_of_a_dot_cargo_home() {
        let td = tempfile::tempdir().unwrap();
        let user_home = td.path().join(".cargo");
        let project = td.path().join("code/proj");
        std::fs::create_dir_all(&user_home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let outside = tempfile::tempdir().unwrap();

        // No config at all: nothing to lose, redirect from anywhere.
        assert!(config_reachable(&user_home, &project));
        assert!(config_reachable(&user_home, outside.path()));

        std::fs::write(user_home.join("config.toml"), "[build]\n").unwrap();
        assert!(config_reachable(&user_home, &project));
        assert!(config_reachable(&user_home, td.path()));
        assert!(!config_reachable(&user_home, outside.path()));
        // A checkout that doesn't exist yet still resolves by its prefix.
        assert!(config_reachable(
            &user_home,
            &td.path().join("later/checkout")
        ));

        // The legacy name counts as a config too.
        std::fs::remove_file(user_home.join("config.toml")).unwrap();
        std::fs::write(user_home.join("config"), "[build]\n").unwrap();
        assert!(!config_reachable(&user_home, outside.path()));

        // A custom home is never found by the walk, which looks only for
        // `<ancestor>/.cargo/config.toml`.
        let custom = td.path().join("opt/cargo");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(custom.join("config.toml"), "[build]\n").unwrap();
        assert!(!config_reachable(&custom, &td.path().join("opt/proj")));
    }

    #[test]
    fn prepare_links_only_the_credentials_that_exist() {
        let (_td, cache_root, source) = roots();
        std::fs::write(
            source.join("credentials.toml"),
            "[registry]\ntoken = \"t\"\n",
        )
        .unwrap();

        prepare(&cache_root, &source, BoxPlatform::host()).unwrap();

        let overlay = overlay(&cache_root, BoxPlatform::host());
        assert!(overlay.is_dir());
        assert_eq!(
            std::fs::read_link(overlay.join("credentials.toml")).unwrap(),
            source.join("credentials.toml")
        );
        assert!(
            overlay.join("credentials").symlink_metadata().is_err(),
            "the legacy name has no source, so no link"
        );
        // Cargo reads the token through the link.
        assert_eq!(
            std::fs::read_to_string(overlay.join("credentials.toml")).unwrap(),
            "[registry]\ntoken = \"t\"\n"
        );
    }

    /// The global config is never linked: cargo resolves a config file's
    /// relative values (`include`, `paths`, `[patch]`, `target-dir`, …)
    /// against the parent of the dir it *found* the file in, unresolved, so a
    /// link here would rebase `~/.cargo/config.toml` onto the cache root. The
    /// real file is reached by cargo's ancestor walk instead (module doc).
    #[test]
    fn prepare_never_links_the_config_and_removes_a_planted_one() {
        let (_td, cache_root, source) = roots();
        std::fs::write(
            source.join("config.toml"),
            "include = [\"company.toml\"]\n[build]\ntarget-dir = \"tgt\"\n",
        )
        .unwrap();
        std::fs::write(source.join("config"), "legacy").unwrap();

        prepare(&cache_root, &source, BoxPlatform::host()).unwrap();
        let overlay = overlay(&cache_root, BoxPlatform::host());
        for item in CONFIG_FILES {
            assert!(
                overlay.join(item).symlink_metadata().is_err(),
                "{item} must not be linked into the overlay"
            );
        }

        // An agent planted a config to steer every run sharing the overlay;
        // the next launch removes it. The user's own files are untouched.
        std::fs::write(overlay.join("config.toml"), "[alias]\nb = \"evil\"\n").unwrap();
        prepare(&cache_root, &source, BoxPlatform::host()).unwrap();
        assert!(overlay.join("config.toml").symlink_metadata().is_err());
        assert!(source.join("config.toml").is_file());
        assert!(source.join("config").is_file());
    }

    #[test]
    fn prepare_repairs_what_the_agent_changed_and_drops_dangling_links() {
        let (_td, cache_root, source) = roots();
        std::fs::write(source.join("credentials.toml"), "a").unwrap();
        std::fs::write(source.join("credentials"), "b").unwrap();
        prepare(&cache_root, &source, BoxPlatform::host()).unwrap();
        let overlay = overlay(&cache_root, BoxPlatform::host());

        // An agent swapped one link for a file of its own, and the user
        // deleted the legacy file since the last launch.
        std::fs::remove_file(overlay.join("credentials.toml")).unwrap();
        std::fs::write(overlay.join("credentials.toml"), "token = \"stolen\"\n").unwrap();
        std::fs::remove_file(source.join("credentials")).unwrap();

        prepare(&cache_root, &source, BoxPlatform::host()).unwrap();

        assert_eq!(
            std::fs::read_link(overlay.join("credentials.toml")).unwrap(),
            source.join("credentials.toml")
        );
        assert!(
            overlay.join("credentials").symlink_metadata().is_err(),
            "a link whose source is gone is removed, not left dangling"
        );
        // The user's own file is untouched throughout.
        assert_eq!(
            std::fs::read_to_string(source.join("credentials.toml")).unwrap(),
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
                .map(|_| s.spawn(|| prepare(&cache_root, &source, BoxPlatform::host())))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for r in results {
            r.unwrap();
        }
        let overlay = overlay(&cache_root, BoxPlatform::host());
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
        std::fs::write(source.join("credentials.toml"), "a").unwrap();
        std::fs::write(source.join("config.toml"), "c").unwrap();
        // The agent replaced the whole overlay with a link to the user's real
        // cargo home. A host that followed it would unlink the user's
        // `config.toml` outright and replace their `credentials.toml` with a
        // self-referential link. The overlay is two levels down (`cargo/<platform>`)
        // and either level can be the one swapped, so both are tried.
        let overlay = overlay(&cache_root, BoxPlatform::host());
        for swapped in [overlay.parent().unwrap(), overlay.as_path()] {
            let _ = std::fs::remove_dir_all(&cache_root);
            std::fs::create_dir_all(swapped.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&source, swapped).unwrap();

            prepare(&cache_root, &source, BoxPlatform::host()).unwrap();

            assert!(
                swapped.symlink_metadata().unwrap().is_dir(),
                "{} is a real directory again",
                swapped.display()
            );
        }
        assert!(
            overlay.symlink_metadata().unwrap().is_dir(),
            "the overlay is a real directory again"
        );
        assert_eq!(
            std::fs::read_to_string(source.join("config.toml")).unwrap(),
            "c",
            "the user's config survived the config sweep"
        );
        assert_eq!(
            std::fs::read_link(overlay.join("credentials.toml")).unwrap(),
            source.join("credentials.toml")
        );
        assert!(
            source
                .join("credentials.toml")
                .symlink_metadata()
                .unwrap()
                .is_file(),
            "the user's credentials are still their regular file"
        );
        assert_eq!(
            std::fs::read_to_string(source.join("credentials.toml")).unwrap(),
            "a"
        );
    }
}
