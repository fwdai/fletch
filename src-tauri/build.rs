use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

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

    generate_extension_glue();
    tauri_build::build()
}

/// Discover extensions under the repo-root `extensions/` directory and generate
/// the glue that wires their backends and migrations into the app crate. Each
/// `extensions/<name>/backend/mod.rs` is compiled into this crate via `#[path]`
/// (so extensions need no Cargo crate of their own), and each
/// `extensions/<name>/migrations/*.sql` is embedded for the runtime migrator.
///
/// Absent extensions (e.g. a fresh public clone with none checked out) simply
/// produce empty glue, so the open-source build compiles with nothing extra.
fn generate_extension_glue() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let ext_root = Path::new(&manifest).join("../extensions");
    let dest = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("extensions_glue.rs");

    // Rerun when extensions are added/removed (directory mtime changes).
    println!("cargo::rerun-if-changed={}", ext_root.display());

    let mut mods = String::new();
    let mut registrations = String::new();
    let mut migrations = String::new();

    if let Ok(entries) = fs::read_dir(&ext_root) {
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();

        // Map each module ident back to the folder that produced it, so two
        // folder names that sanitize to the same ident (e.g. `my-ext` and
        // `my.ext` -> `my_ext`) fail with a clear message here instead of a
        // cryptic "module ext_my_ext is already defined" later.
        let mut seen_idents: HashMap<String, String> = HashMap::new();

        for dir in dirs {
            let name = dir.file_name().unwrap().to_string_lossy().to_string();
            let ident = sanitize_ident(&name);

            if let Some(prev) = seen_idents.insert(ident.clone(), name.clone()) {
                println!(
                    "cargo::error=extension folders '{prev}' and '{name}' both map to module \
                     identifier 'ext_{ident}' after sanitization; rename one so they differ by \
                     more than non-alphanumeric characters."
                );
                continue;
            }

            // Watch the extension dir so adding a backend/ or migrations/ subdir
            // (a new file directly under it) retriggers codegen; the per-file
            // watches below only cover files that already existed last build.
            println!("cargo::rerun-if-changed={}", dir.display());

            let backend = dir.join("backend").join("mod.rs");
            if backend.exists() {
                let abs = backend.canonicalize().unwrap();
                let abs = abs.to_string_lossy();
                println!("cargo::rerun-if-changed={abs}");
                mods.push_str(&format!("#[path = {abs:?}]\nmod ext_{ident};\n"));
                registrations.push_str(&format!(
                    "    {{ let mut r = api.scope({name:?}); ext_{ident}::register(&mut r); }}\n"
                ));
            }

            let migdir = dir.join("migrations");
            if migdir.is_dir() {
                // Watch the dir itself so a newly added .sql file is detected,
                // not just edits to ones already known last build.
                println!("cargo::rerun-if-changed={}", migdir.display());
                let mut files: Vec<PathBuf> = fs::read_dir(&migdir)
                    .map(|rd| {
                        rd.filter_map(|e| e.ok())
                            .map(|e| e.path())
                            .filter(|p| p.extension().is_some_and(|x| x == "sql"))
                            .collect()
                    })
                    .unwrap_or_default();
                files.sort();
                for f in files {
                    let abs = f.canonicalize().unwrap();
                    let abs = abs.to_string_lossy();
                    println!("cargo::rerun-if-changed={abs}");
                    let fname = f.file_name().unwrap().to_string_lossy().to_string();
                    migrations.push_str(&format!(
                        "    ExtMigration {{ extension: {name:?}, name: {fname:?}, sql: include_str!({abs:?}) }},\n"
                    ));
                }
            }
        }
    }

    // Pasted into `crate::extensions` via include!, so types are referenced
    // unqualified (same module). Empty bodies are valid no-ops.
    let code = format!(
        "// @generated by build.rs — extension discovery. Do not edit.\n\
         {mods}\n\
         #[allow(unused_variables)]\n\
         pub(crate) fn register_all(api: &mut ExtensionApi) {{\n{registrations}}}\n\n\
         pub(crate) fn extension_migrations() -> Vec<ExtMigration> {{\n    vec![\n{migrations}    ]\n}}\n"
    );
    fs::write(&dest, code).expect("write extensions_glue.rs");
}

/// Turn an extension folder name into a valid Rust module identifier
/// (`demo.local` -> `demo_local`).
fn sanitize_ident(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}
