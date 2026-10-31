//! Where a project's agents run when the engine choice is automatic: the
//! `sandbox.placement` project setting, and the one thing Fletch decides on
//! its own — a project that builds a macOS or iOS app runs on this Mac,
//! since a Linux guest cannot build or run it.

use std::path::Path;

/// The project setting. Absent is [`Placement::Auto`].
pub const KEY: &str = "sandbox.placement";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// A box when one can boot, unless the project looks native to the Mac.
    Auto,
    /// Always the macOS sandbox.
    Host,
    /// Always a box, waiting for the image if it is not in yet: the user's
    /// answer to a native-looking project that is not one.
    Box,
}

impl Placement {
    pub fn parse(raw: Option<&str>) -> Placement {
        match raw.map(str::trim) {
            Some("host") => Placement::Host,
            Some("box") => Placement::Box,
            _ => Placement::Auto,
        }
    }

    pub fn as_setting(self) -> &'static str {
        match self {
            Placement::Auto => "auto",
            Placement::Host => "host",
            Placement::Box => "box",
        }
    }
}

/// The placement of the project at `repo` whose setting is `raw`: the stored
/// choice, or for automatic the detection — `Host` when the repository looks
/// like a macOS or iOS app, `Auto` otherwise.
pub fn resolve(raw: Option<&str>, repo: &Path) -> Placement {
    match Placement::parse(raw) {
        Placement::Auto if looks_native(repo) => Placement::Host,
        placement => placement,
    }
}

/// Whether the repository at `repo` builds a macOS or iOS app, from its root
/// alone: an Xcode project or workspace, a CocoaPods `Podfile`, a Tauri app,
/// or a Swift package whose manifest names an Apple platform. Cheap — one
/// directory listing and at most one small file read — since the spawn path
/// asks on every automatic placement.
pub fn looks_native(repo: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(repo) else {
        return false;
    };
    let mut package_swift = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".xcodeproj") || name.ends_with(".xcworkspace") || name == "Podfile" {
            return true;
        }
        if name == "Package.swift" {
            package_swift = true;
        }
    }
    if repo.join("src-tauri").join("tauri.conf.json").is_file()
        || repo.join("tauri.conf.json").is_file()
    {
        return true;
    }
    if package_swift {
        // `platforms: [.macOS(...), .iOS(...)]`: a package for Apple platforms.
        // A package with no platforms line is a library, buildable anywhere.
        if let Ok(manifest) = std::fs::read_to_string(repo.join("Package.swift")) {
            return manifest.contains(".macOS(")
                || manifest.contains(".iOS(")
                || manifest.contains(".visionOS(")
                || manifest.contains(".watchOS(")
                || manifest.contains(".tvOS(");
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_choice_wins_and_absent_is_auto() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Placement::parse(None), Placement::Auto);
        assert_eq!(Placement::parse(Some(" host ")), Placement::Host);
        assert_eq!(Placement::parse(Some("box")), Placement::Box);
        assert_eq!(Placement::parse(Some("vm")), Placement::Auto);
        assert_eq!(resolve(None, dir.path()), Placement::Auto);
        std::fs::create_dir(dir.path().join("App.xcodeproj")).unwrap();
        assert_eq!(resolve(None, dir.path()), Placement::Host);
        // The user said boxes: detection steps aside.
        assert_eq!(resolve(Some("box"), dir.path()), Placement::Box);
    }

    #[test]
    fn native_apps_are_told_from_their_root() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!looks_native(dir.path()));
        assert!(!looks_native(&dir.path().join("missing")));

        std::fs::write(
            dir.path().join("Package.swift"),
            "let package = Package(name: \"x\")",
        )
        .unwrap();
        assert!(
            !looks_native(dir.path()),
            "a plain Swift package builds anywhere"
        );
        std::fs::write(
            dir.path().join("Package.swift"),
            "platforms: [.macOS(.v13), .iOS(.v16)]",
        )
        .unwrap();
        assert!(looks_native(dir.path()));
        std::fs::remove_file(dir.path().join("Package.swift")).unwrap();

        std::fs::create_dir_all(dir.path().join("src-tauri")).unwrap();
        std::fs::write(dir.path().join("src-tauri/tauri.conf.json"), "{}").unwrap();
        assert!(looks_native(dir.path()));
        std::fs::remove_dir_all(dir.path().join("src-tauri")).unwrap();

        std::fs::write(dir.path().join("Podfile"), "").unwrap();
        assert!(looks_native(dir.path()));
    }
}
