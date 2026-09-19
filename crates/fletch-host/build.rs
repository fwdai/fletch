//! One job: put the compile-time target triple where the binary can read it.
//!
//! The release job names every host archive `fletch-host-<version>-<target>`
//! (.github/workflows/release.yml), so `fletch-host update` has to ask for its
//! own triple by name. Cargo tells build scripts the target and tells the crate
//! itself nothing, which is why this file exists.

fn main() {
    let target = std::env::var("TARGET").expect("cargo always sets TARGET for a build script");
    println!("cargo:rustc-env=TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
