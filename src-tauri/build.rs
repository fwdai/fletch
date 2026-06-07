fn main() {
    // OAuth client keys are embedded at compile time via `option_env!`. Cargo
    // doesn't track those env vars on its own, so declare them here — otherwise
    // a cached/incremental build could ship a stale or empty value.
    for key in [
        "QUORUM_GITHUB_CLIENT_ID",
        "QUORUM_GOOGLE_CLIENT_ID",
        "QUORUM_GOOGLE_CLIENT_SECRET",
    ] {
        println!("cargo::rerun-if-env-changed={key}");
    }
    tauri_build::build()
}
