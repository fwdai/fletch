// OAuth client keys are baked into the binary at compile time via `option_env!`
// (see src-tauri/src/oauth.rs). CI supplies them as process env from repository
// secrets (see .github/workflows/release.yml). For local dev they'd otherwise
// be empty — making "Connect GitHub"/Google sign-in report "not configured" —
// so we also load a repo-root `.env` here and forward these keys into the
// compile. A value already present in the environment always wins, so CI is
// unaffected and a developer can still override `.env` with a shell export.
const CONFIG_KEYS: [&str; 4] = [
    "QUORUM_GITHUB_CLIENT_ID",
    "QUORUM_GOOGLE_CLIENT_ID",
    "QUORUM_GOOGLE_CLIENT_SECRET",
    // Gates the whole PostHog pipeline — usage telemetry AND in-app feedback
    // (see `telemetry.rs`). Without it a local build can only ever exercise the
    // "no endpoint configured" path. `QUORUM_POSTHOG_HOST` is deliberately not
    // forwarded: it already defaults to PostHog US cloud, and a self-hosted
    // instance is a CI-env concern, not a local-dev one.
    "QUORUM_POSTHOG_KEY",
];

fn main() {
    // build.rs runs with the package dir (src-tauri/) as CWD; the shared local
    // `.env` lives at the repo root, one level up.
    let dotenv = std::path::Path::new("../.env");
    // Track the path unconditionally (even when absent), so *creating* `.env`
    // later reruns this script — otherwise a direct `cargo`/rust-analyzer build
    // could keep the old empty `option_env!` values until an unrelated input
    // changes.
    println!("cargo::rerun-if-changed=../.env");
    if dotenv.exists() {
        if let Ok(contents) = std::fs::read_to_string(dotenv) {
            for (key, value) in parse_env(&contents) {
                if CONFIG_KEYS.contains(&key.as_str()) && std::env::var_os(&key).is_none() {
                    println!("cargo::rustc-env={key}={value}");
                }
            }
        }
    }

    // Cargo doesn't track these env vars on its own, so declare them — otherwise
    // a cached/incremental build could ship a stale or empty value.
    for key in CONFIG_KEYS {
        println!("cargo::rerun-if-env-changed={key}");
    }

    link_clang_runtime();

    tauri_build::build()
}

/// whisper.cpp's Metal backend guards its residency-set path on
/// `@available(macOS 15.0, ...)` (`ggml/src/ggml-metal/ggml-metal-device.m`),
/// which clang lowers to a call to `___isPlatformVersionAtLeast`. That helper
/// lives in clang's own runtime library, `libclang_rt.osx.a`, and rustc drives
/// the final link with `-nodefaultlibs` — so nothing puts it on the link line
/// and the `fletch` binary fails to link with three undefined symbols.
///
/// It only bites when the SDK is >= 15.0 (which turns the path on) *and* the
/// deployment target is below it (which makes the check a runtime one rather
/// than a folded constant) — i.e. every release build, since
/// `minimumSystemVersion` is 13.0. Only the arm64 slice is affected; the guard
/// is `!TARGET_CPU_X86_64`, so x86_64 compiles the path out. Linux CI never sees
/// it either: `whisper-rs` is macOS-only (see Cargo.toml).
///
/// The archive is linked without a `static=` kind on purpose, so the linker
/// pulls in only the availability object rather than bundling all of
/// compiler-rt into the rlib and colliding with Rust's own
/// `compiler_builtins`.
fn link_clang_runtime() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    // Ask the same compiler that will drive the link (see the `cc` invocation in
    // the failing linker output) where its runtime lives, rather than guessing a
    // versioned path under Xcode.
    println!("cargo::rerun-if-env-changed=CC");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());

    let dir = match std::process::Command::new(&cc)
        .arg("-print-runtime-dir")
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => {
            println!("cargo::warning=`{cc} -print-runtime-dir` failed; not linking clang_rt.osx");
            return;
        }
    };

    // clang prints the path it *would* use, whether or not it exists, so check
    // for the archive itself. Warn rather than panic: a toolchain without it can
    // still build everything that doesn't need the availability helper.
    if dir.is_empty()
        || !std::path::Path::new(&dir)
            .join("libclang_rt.osx.a")
            .exists()
    {
        println!("cargo::warning=libclang_rt.osx.a not found in `{dir}`; not linking clang_rt.osx");
        return;
    }

    println!("cargo::rustc-link-search=native={dir}");
    println!("cargo::rustc-link-lib=clang_rt.osx");
}

/// Minimal `.env` parser: `KEY=VALUE` lines, optional `export ` prefix and
/// surrounding quotes; `#` comments and blank lines skipped. Only the plain
/// scalar values we forward (client ids/secrets) matter — lines we can't parse
/// are ignored rather than erroring the build.
fn parse_env(contents: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let unquoted = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        out.push((key.trim().to_string(), unquoted.to_string()));
    }
    out
}
