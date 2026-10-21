//! The redirect table: which env var points which toolchain where under a
//! project's root.

use std::path::Path;

use Keying::{PerPlatform, Shared};

/// Whether a row's contents differ per platform. A macOS agent and a Linux
/// container of one project share the root, so a per-platform row lands in a
/// [`BoxPlatform::key`] subdir of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Keying {
    Shared,
    PerPlatform,
}

/// `(env var, subdir under the root, keying)` for every toolchain we redirect.
///
/// Adding a row is the *whole* cost of supporting a new toolchain: no sandbox
/// policy change, because the project's root is already granted. Getting a row
/// wrong is safe in the only direction that matters: an env var a tool ignores
/// leaves it writing its default path, which fails closed exactly as before
/// the redirect existed, never a widened sandbox.
///
/// **Only ever redirect a variable whose target is purely a cache.** A variable
/// naming a toolchain *home* (`RUSTUP_HOME`, `CARGO_HOME`, `GEM_HOME`,
/// `GRADLE_USER_HOME`, `COMPOSER_HOME`, …) points at a directory that also
/// holds user config and installed artifacts, so redirecting it doesn't give
/// the toolchain a writable scratch dir, it *hides the user's existing
/// installation*. `RUSTUP_HOME` is the clearest case: it contains the
/// toolchains themselves and the `settings.toml` naming the default, so
/// pointing it at a fresh empty dir leaves `cargo` with no compiler.
/// `COMPOSER_HOME` and `GRADLE_USER_HOME` are quieter versions of the same bug:
/// they hold `auth.json` / `gradle.properties`, so redirecting silently drops
/// registry credentials. Those all once lived in this table and were removed.
/// Reads are unrestricted sandbox-wide, so leaving a home alone costs an
/// installed toolchain nothing; only writing to it fails, and failing loudly
/// beats silently pretending the user has no Rust.
///
/// The cost is that toolchains with no cache-only variable are **not served by
/// this table**: Ruby/JVM dependency installs still fail closed. Cargo is the
/// one home-only toolchain served anyway, by a different mechanism:
/// [`crate::sandbox::cargo_home`] builds a Fletch-owned `CARGO_HOME` under the
/// root with the user's credentials linked in.
///
/// Verified empirically on macOS against the installed toolchain at the time
/// of writing: bun 1.3, npm 10.9, pnpm 10.33, yarn 1.22, deno 2.6, pip 25.3,
/// uv 0.11, gem 3.5, playwright 1.58. The rest are documentation-derived, safe
/// per the fail-closed note above but unproven. `pnpm` is the cautionary row:
/// it does **not** read `PNPM_STORE_DIR` (silently ignored); it takes config
/// through npm's env convention, hence `npm_config_store_dir`.
///
/// Headless browsers are downloaded tools like any package cache, so they ride
/// the same rows. They only ever run inside sandboxes: the host's own
/// Playwright keeps its default store, so a Chromium an agent downloads (or
/// tampers with) here never runs on the host.
///
/// Three rows are [`PerPlatform`]. Playwright names its folders without an OS
/// (`chromium-1208/` plus an `INSTALLATION_COMPLETE` marker), so a Linux box
/// would see the Mac install, skip its download, and fail to launch. Poetry's
/// cache holds `virtualenvs/`, which are per-platform builds. npm's cache holds
/// `_npx/`, the packages `npx` installed with only the one platform's optional
/// dependency (`@esbuild/darwin-arm64`), which `npx` on the other platform
/// would reuse rather than install its own. The rest are safe to share:
/// pnpm/bun/yarn store tarballs, go's `GOCACHE`, uv and Puppeteer key by
/// platform internally, and pip checks wheel tags.
pub(super) const TOOLCHAIN_CACHE_VARS: &[(&str, &str, Keying)] = &[
    // JavaScript / TypeScript
    ("BUN_INSTALL_CACHE_DIR", "bun", Shared),
    ("npm_config_cache", "npm", PerPlatform), // _npx/ holds installed packages
    ("npm_config_store_dir", "pnpm", Shared), // NOT PNPM_STORE_DIR, see above
    ("YARN_CACHE_FOLDER", "yarn", Shared),
    ("DENO_DIR", "deno", Shared),
    // Go (Rust is served by the `cargo_home` overlay: a bare CARGO_HOME or
    // RUSTUP_HOME redirect would hide the user's config and toolchains.)
    ("GOMODCACHE", "go/mod", Shared),
    ("GOCACHE", "go/build", Shared),
    // Python
    ("PIP_CACHE_DIR", "pip", Shared),
    ("UV_CACHE_DIR", "uv", Shared),
    ("POETRY_CACHE_DIR", "poetry", PerPlatform), // virtualenvs/ are per-platform
    // Ruby: the spec cache only; GEM_HOME is where gems are *installed*.
    ("GEM_SPEC_CACHE", "gem/spec-cache", Shared),
    // PHP: the cache only; COMPOSER_HOME holds auth.json.
    ("COMPOSER_CACHE_DIR", "composer/cache", Shared),
    // .NET: the global packages folder is a restore target, not a config home.
    ("NUGET_PACKAGES", "nuget", Shared),
    // Browsers: downloaded binaries, executed only inside sandboxes.
    ("PLAYWRIGHT_BROWSERS_PATH", "playwright", PerPlatform), // folders carry no OS
    ("PUPPETEER_CACHE_DIR", "puppeteer", Shared),
    // Elixir is absent for the same reason as the Rust/Ruby/PHP homes: MIX_HOME
    // and HEX_HOME hold archives and `hex.config` (which can carry an API key).
];

/// The environment redirecting every toolchain in [`TOOLCHAIN_CACHE_VARS`]
/// into `root`, for a child running on `platform`.
///
/// Note what is **not** here: `XDG_DATA_HOME` and `XDG_CONFIG_HOME`. Both are
/// load-bearing for provider state resolution
/// ([`crate::sandbox::policy::opencode_data_dir`] and the host-side reader in
/// `agent::providers::opencode`), which resolves them from the *host* env, so
/// setting them for the child would move opencode's session store out from
/// under the host reader. `XDG_CACHE_HOME` is safe but pointless: the policy's
/// scratch grants already cover `~/.cache` and `~/Library/Caches`, so
/// XDG-conformant tools never fail in the first place. This table is for the
/// toolchains that *don't* follow XDG.
pub fn toolchain_cache_env(root: &Path, platform: BoxPlatform) -> Vec<(String, String)> {
    TOOLCHAIN_CACHE_VARS
        .iter()
        .map(|(var, subdir, keying)| {
            let mut dir = root.join(subdir);
            if *keying == PerPlatform {
                dir.push(platform.key());
            }
            ((*var).to_string(), dir.to_string_lossy().into_owned())
        })
        .collect()
}

/// The OS and CPU a confined child runs on, keying the [`PerPlatform`] rows
/// and the cargo overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxPlatform {
    os: &'static str,
    arch: &'static str,
}

impl BoxPlatform {
    /// The host itself: seatbelt agents and Run commands.
    pub fn host() -> Self {
        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        }
    }

    /// A Linux container, which runs on the host's CPU.
    pub fn linux() -> Self {
        Self {
            os: "linux",
            arch: std::env::consts::ARCH,
        }
    }

    /// The subdir key, e.g. `macos-aarch64` or `linux-x86_64`.
    pub fn key(&self) -> String {
        format!("{}-{}", self.os, self.arch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const ROOT: &str = "/Users/u/.fletch/toolchain-cache/p1";

    /// Every variable points strictly inside the granted root, once each. A
    /// row escaping the root would ask the toolchain to write somewhere the
    /// profile doesn't grant: a silent fail-closed that looks exactly like the
    /// bug this mechanism removes.
    #[test]
    fn every_row_points_inside_the_root_once() {
        let root = Path::new(ROOT);
        for platform in [BoxPlatform::host(), BoxPlatform::linux()] {
            let env = toolchain_cache_env(root, platform);
            assert_eq!(env.len(), TOOLCHAIN_CACHE_VARS.len());
            for (var, value) in &env {
                let p = PathBuf::from(value);
                assert!(
                    p.starts_with(root) && p != root,
                    "{var} points outside the granted root: {value}"
                );
            }
        }
        let mut names: Vec<&str> = TOOLCHAIN_CACHE_VARS.iter().map(|(v, _, _)| *v).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate env var in the table");
    }

    /// A per-platform row (Playwright) must diverge between a macOS agent and a
    /// Linux container while every shared row stays shared; otherwise the Linux
    /// box trusts the Mac's install marker.
    #[test]
    fn per_platform_rows_split_host_and_linux_and_nothing_else() {
        let root = Path::new(ROOT);
        let host = toolchain_cache_env(root, BoxPlatform::host());
        let linux = toolchain_cache_env(root, BoxPlatform::linux());
        let arch = std::env::consts::ARCH;

        let get = |env: &[(String, String)], var: &str| {
            env.iter().find(|(v, _)| v == var).unwrap().1.clone()
        };
        let pw = "PLAYWRIGHT_BROWSERS_PATH";
        assert_eq!(
            get(&linux, pw),
            root.join(format!("playwright/linux-{arch}"))
                .to_string_lossy()
        );
        assert_eq!(
            get(&host, pw),
            root.join(format!("playwright/{}-{arch}", std::env::consts::OS))
                .to_string_lossy()
        );

        for (((var, h), (_, l)), (_, _, keying)) in
            host.iter().zip(&linux).zip(TOOLCHAIN_CACHE_VARS)
        {
            if *keying == Shared {
                assert_eq!(h, l, "{var} is shared but differs across platforms");
            } else if cfg!(not(target_os = "linux")) {
                assert_ne!(h, l, "{var} is per-platform but shared across platforms");
            }
        }
    }

    /// The redirect must never claim a variable naming a toolchain *home*.
    ///
    /// A home holds installed artifacts and user config, not just cache, so
    /// pointing one at an empty dir hides the user's installation rather than
    /// giving the toolchain scratch space. `RUSTUP_HOME` is the sharpest case;
    /// `COMPOSER_HOME` / `GRADLE_USER_HOME` / `BUNDLE_USER_HOME` are the quiet
    /// version: they carry registry credentials that would silently vanish.
    ///
    /// Every one of these was in the table at one point. Where a toolchain
    /// still needs somewhere writable, the answer is a redirected home that
    /// *links the user's credentials back in*, built host-side before launch
    /// ([`crate::sandbox::cargo_home`] for cargo), never a bare row here,
    /// and never a cache island under the real home (its unpacked sources are
    /// code the user's next host build runs).
    #[test]
    fn redirect_never_claims_a_toolchain_home() {
        const FORBIDDEN: &[&str] = &[
            "RUSTUP_HOME",
            "CARGO_HOME",
            "GEM_HOME",
            "BUNDLE_USER_HOME",
            "GRADLE_USER_HOME",
            "COMPOSER_HOME",
            "MIX_HOME",
            "HEX_HOME",
            "DOTNET_CLI_HOME",
        ];
        for (var, _, _) in TOOLCHAIN_CACHE_VARS {
            assert!(
                !FORBIDDEN.contains(var),
                "{var} names a toolchain home: redirecting it hides the user's \
                 installed toolchain and config; build an overlay that links them \
                 in, as cargo_home does"
            );
        }
    }

    /// The redirect must never claim a variable that provider-state resolution
    /// depends on. `XDG_DATA_HOME`/`XDG_CONFIG_HOME` resolve opencode's session
    /// store and config, and the *host* side reads them too, from the host env;
    /// redirecting them for the child would break transcript ingest silently,
    /// with the agent still appearing to work. `CODEX_HOME` and
    /// `CLAUDE_CONFIG_DIR` are the same hazard for their providers.
    #[test]
    fn redirect_never_claims_a_provider_state_var() {
        const FORBIDDEN: &[&str] = &[
            "XDG_DATA_HOME",
            "XDG_CONFIG_HOME",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
        ];
        for (var, _, _) in TOOLCHAIN_CACHE_VARS {
            assert!(
                !FORBIDDEN.contains(var),
                "{var} resolves provider state: redirecting it breaks transcript/auth reads"
            );
        }
    }
}
