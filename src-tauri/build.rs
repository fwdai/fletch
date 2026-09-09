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
    build_speech_bridge();

    tauri_build::build()
}

/// The Swift half of dictation's default engine (see `swift/SpeechBridge.swift`
/// and `docs/dictation.md`). `SpeechAnalyzer` is Swift-only, so this compiles
/// the one Swift file into a static library with `swiftc` and links it in.
///
/// The Swift objects carry autolink entries (`LC_LINKER_OPTION`) for
/// `swiftCore`, `Foundation`, `Speech` and friends, so the only thing the Rust
/// link needs beyond the archive itself is a search path to the OS runtime's
/// stubs in the SDK — nothing is bundled; the Swift runtime has shipped with
/// macOS since 10.14.4.
fn build_speech_bridge() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    println!("cargo::rerun-if-changed={SPEECH_BRIDGE}");
    // `xcrun` honours both; a switch of SDK or deployment target must rebuild.
    println!("cargo::rerun-if-env-changed=SDKROOT");
    println!("cargo::rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");

    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        other => panic!("dictation: no Swift target for architecture {other:?}"),
    };
    let deployment = std::env::var("MACOSX_DEPLOYMENT_TARGET")
        .unwrap_or_else(|_| MACOS_DEPLOYMENT_TARGET.to_string());

    let sdk = xcrun(&["--sdk", "macosx", "--show-sdk-path"]);
    let sdk_version = xcrun(&["--sdk", "macosx", "--show-sdk-version"]);
    let sdk_major: u32 = sdk_version
        .split('.')
        .next()
        .and_then(|major| major.parse().ok())
        .unwrap_or(0);
    assert!(
        sdk_major >= 26,
        "dictation: the Swift bridge needs the macOS 26 SDK (Xcode 26 or newer); \
         `xcrun --show-sdk-path` found SDK {sdk_version} at {sdk}. Select a newer Xcode \
         with `sudo xcode-select -s /Applications/Xcode.app`."
    );
    let swiftc = xcrun(&["--sdk", "macosx", "--find", "swiftc"]);

    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let archive = format!("{out_dir}/libfletch_speech.a");
    let status = std::process::Command::new(&swiftc)
        .args([
            "-emit-library",
            "-static",
            "-parse-as-library",
            "-module-name",
            "FletchSpeech",
            "-swift-version",
            "5",
            "-O",
            "-target",
            &format!("{arch}-apple-macos{deployment}"),
            "-sdk",
            &sdk,
            SPEECH_BRIDGE,
            "-o",
            &archive,
        ])
        .status();
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            panic!("dictation: `{swiftc}` failed with {status} compiling {SPEECH_BRIDGE}")
        }
        Err(e) => panic!("dictation: couldn't run `{swiftc}`: {e}"),
    }

    println!("cargo::rustc-link-search=native={out_dir}");
    println!("cargo::rustc-link-lib=static=fletch_speech");
    println!("cargo::rustc-link-search=native={sdk}/usr/lib/swift");
    // `libswift_Concurrency` is the one runtime library the objects reference
    // as `@rpath/…` rather than by absolute path (it is the back-deployable
    // one), so without an rpath the binary aborts at load with "Library not
    // loaded". `swiftc` adds this same rpath to everything it links on macOS.
    println!("cargo::rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}

/// Run `xcrun` and return its trimmed stdout, or stop the build with a message
/// that says what to install. `xcrun` is the one tool that knows which Xcode
/// (or Command Line Tools) is selected, so it locates both the SDK and
/// `swiftc` rather than us guessing at versioned paths.
fn xcrun(args: &[&str]) -> String {
    match std::process::Command::new("xcrun").args(args).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Ok(out) => panic!(
            "dictation: `xcrun {}` failed: {}\nBuilding Fletch for macOS needs Xcode 26 or \
             newer (its Swift compiler builds the dictation bridge, {SPEECH_BRIDGE}). \
             Install Xcode and select it with `sudo xcode-select -s /Applications/Xcode.app`, \
             or install the Command Line Tools with `xcode-select --install`.",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => panic!(
            "dictation: couldn't run `xcrun`: {e}\nBuilding Fletch for macOS needs Xcode 26 or \
             newer (its Swift compiler builds the dictation bridge, {SPEECH_BRIDGE}). \
             Install Xcode and select it with `sudo xcode-select -s /Applications/Xcode.app`, \
             or install the Command Line Tools with `xcode-select --install`."
        ),
    }
}

/// The dictation bridge, relative to this package (build scripts run with the
/// package directory as CWD).
const SPEECH_BRIDGE: &str = "swift/SpeechBridge.swift";

/// Deployment target for the bridge when the build doesn't set one — the same
/// `13.0` as `bundle.macOS.minimumSystemVersion` in `tauri.conf.json`. It only
/// decides which Swift runtime the objects expect at link time; the code itself
/// is gated at runtime on macOS 26 with `#available`.
const MACOS_DEPLOYMENT_TARGET: &str = "13.0";

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
