// OAuth client keys are baked into the binary at compile time via `option_env!`
// (see src-tauri/src/oauth.rs). CI supplies them as process env from repository
// secrets (see .github/workflows/release.yml). For local dev they'd otherwise
// be empty — making "Connect GitHub"/Google sign-in report "not configured" —
// so we also load a repo-root `.env` here and forward these keys into the
// compile. A value already present in the environment always wins, so CI is
// unaffected and a developer can still override `.env` with a shell export.
const CONFIG_KEYS: [&str; 3] = [
    "QUORUM_GITHUB_CLIENT_ID",
    "QUORUM_GOOGLE_CLIENT_ID",
    "QUORUM_GOOGLE_CLIENT_SECRET",
];

fn main() {
    // build.rs runs with the package dir (src-tauri/) as CWD; the shared local
    // `.env` lives at the repo root, one level up.
    let dotenv = std::path::Path::new("../.env");
    if dotenv.exists() {
        println!("cargo::rerun-if-changed=../.env");
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

    tauri_build::build()
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
