//! Project icon detection (docs/project-surfaces-plan.md §3.6): a shallow,
//! deterministic scan of a project's repositories for the image that already
//! stands for it, ranked so the most deliberate choice comes first:
//!
//! 1. the app icon the build names — Tauri, Electron, an Xcode `AppIcon`,
//!    Android's launcher icon, Flutter's launcher-icon config;
//! 2. a web manifest's largest icon, Next's `app/icon.*`, `index.html`'s
//!    `<link rel="icon">`;
//! 3. `favicon.svg` / `.png`, `apple-touch-icon.png`, then `favicon.ico`;
//! 4. `logo.*` in `public/`, `assets/`, `static/` or `docs/`, and the first
//!    local image the README shows.
//!
//! An ICO is taken only when nothing else was found. Pure file reads — no
//! network, no agent, never a checkout — so it runs when a project is added or
//! relocated and on the picker's "Detect again" ([`store`]).

mod image;
pub mod store;

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub use store::*;

/// How deep the walk lists: `src-tauri/icons/icon.png` is three components.
const MAX_DEPTH: usize = 3;
/// A bound for an enormous tree (a data folder at depth two).
const MAX_ENTRIES: usize = 20_000;
/// Dependencies and build output: never a project's own icon. Electron's
/// `build/icon.*` is read by its fixed path instead.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    "Pods",
];
/// The extensions an icon may have. `.icns` is not one: the webview cannot
/// draw it.
const IMAGE_EXTS: &[&str] = &["png", "svg", "ico", "jpg", "jpeg", "webp", "gif"];
/// A file larger than this is not an icon worth copying.
pub(crate) const MAX_BYTES: u64 = 1 << 20;

/// One image the scan found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    /// Repo-relative, `/`-separated — what `icon.source` stores after
    /// `detected:`. A secondary repo's paths start with its folder name.
    pub path: String,
    /// What found it, for the picker's tooltip.
    pub source: &'static str,
    /// Pixel width, when the file says so cheaply (a PNG's header).
    pub width: Option<u32>,
    /// The file, canonical and inside the repository.
    #[serde(skip)]
    pub file: PathBuf,
}

/// Every candidate in a project's repositories, best first: the primary
/// repo's, then each other repo's, in the project's order.
pub fn scan_project(repos: &[PathBuf]) -> Vec<Candidate> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (i, repo) in repos.iter().enumerate() {
        let prefix = match repo.file_name() {
            Some(name) if i > 0 => format!("{}/", name.to_string_lossy()),
            _ => String::new(),
        };
        for mut c in scan(repo) {
            if seen.insert(c.file.clone()) {
                c.path = format!("{prefix}{}", c.path);
                out.push(c);
            }
        }
    }
    out
}

/// The ranked candidates in one repository.
pub fn scan(repo: &Path) -> Vec<Candidate> {
    let Ok(root) = repo.canonicalize() else {
        return Vec::new();
    };
    let tree = Tree::walk(root);
    let mut found = Vec::new();
    tree.tauri(&mut found);
    tree.electron(&mut found);
    tree.xcode(&mut found);
    tree.android(&mut found);
    tree.flutter(&mut found);
    tree.manifests(&mut found);
    tree.next_app(&mut found);
    tree.index_html(&mut found);
    tree.favicons(&mut found);
    tree.logos(&mut found);
    tree.readme(&mut found);

    let mut seen = HashSet::new();
    found.retain(|c| seen.insert(c.file.clone()));
    // An ICO only when nothing else was found: the stable sort keeps the rank
    // within each half.
    found.sort_by_key(|c| ext(&c.path) == "ico");
    found
}

/// A repository as the rules read it: its entries to [`MAX_DEPTH`], listed
/// once in name order, plus reads by path.
struct Tree {
    root: PathBuf,
    files: Vec<String>,
    dirs: Vec<String>,
}

impl Tree {
    fn walk(root: PathBuf) -> Self {
        let mut tree = Tree {
            root,
            files: Vec::new(),
            dirs: Vec::new(),
        };
        tree.list("", 1);
        tree
    }

    fn list(&mut self, rel: &str, depth: usize) {
        let Ok(entries) = std::fs::read_dir(self.root.join(rel)) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if self.files.len() + self.dirs.len() >= MAX_ENTRIES {
                return;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            // Not followed: a symlink can point out of the repository.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_file() {
                self.files.push(path);
            } else if kind.is_dir() && !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str())
            {
                self.dirs.push(path.clone());
                if depth < MAX_DEPTH {
                    self.list(&path, depth + 1);
                }
            }
        }
    }

    /// `rel` as a candidate: an image file no larger than [`MAX_BYTES`] that
    /// resolves inside the repository.
    fn candidate(&self, rel: &str, source: &'static str) -> Option<Candidate> {
        let rel = clean(rel)?;
        if !IMAGE_EXTS.contains(&ext(&rel).as_str()) {
            return None;
        }
        let file = self.root.join(&rel).canonicalize().ok()?;
        let meta = std::fs::metadata(&file).ok()?;
        if !file.starts_with(&self.root) || !meta.is_file() || meta.len() > MAX_BYTES {
            return None;
        }
        Some(Candidate {
            width: image::png_width_of(&file),
            path: rel,
            source,
            file,
        })
    }

    /// The widest of `paths` that is a candidate; the first on a tie.
    fn widest(
        &self,
        paths: impl IntoIterator<Item = String>,
        source: &'static str,
    ) -> Option<Candidate> {
        paths
            .into_iter()
            .filter_map(|p| self.candidate(&p, source))
            .fold(None, |best: Option<Candidate>, c| match &best {
                Some(b) if b.width >= c.width => best,
                _ => Some(c),
            })
    }

    fn read_text(&self, rel: &str) -> Option<String> {
        let path = self.root.join(rel);
        if std::fs::metadata(&path).ok()?.len() > MAX_BYTES {
            return None;
        }
        std::fs::read_to_string(path).ok()
    }

    fn read_json(&self, rel: &str) -> Option<serde_json::Value> {
        serde_json::from_str(&self.read_text(rel)?).ok()
    }

    /// The names of the files directly in `rel`, sorted (for folders past the
    /// walk's depth).
    fn children(&self, rel: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.root.join(rel)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn files_named<'a>(&'a self, names: &'a [&str]) -> impl Iterator<Item = &'a String> + 'a {
        self.files
            .iter()
            .filter(move |f| names.iter().any(|n| *n == name_of(f)))
    }

    // ── 1. App icons the build names ─────────────────────────────────────

    /// `tauri.conf.json`'s `bundle.icon` (v2; `tauri.bundle.icon` in v1), the
    /// widest PNG it lists, else the widest PNG in the `icons/` beside it.
    fn tauri(&self, out: &mut Vec<Candidate>) {
        for conf in ["src-tauri/tauri.conf.json", "tauri.conf.json"] {
            let Some(json) = self.read_json(conf) else {
                continue;
            };
            let base = parent_of(conf);
            let listed = ["/bundle/icon", "/tauri/bundle/icon"]
                .iter()
                .filter_map(|p| json.pointer(p)?.as_array())
                .flatten()
                .filter_map(|v| v.as_str())
                .filter(|p| ext(p) == "png")
                .map(|p| join(base, p))
                .collect::<Vec<_>>();
            let icons = join(base, "icons");
            let found = self.widest(listed, "Tauri app icon").or_else(|| {
                let pngs = self
                    .files
                    .iter()
                    .filter(|f| parent_of(f) == icons && ext(f) == "png")
                    .cloned();
                self.widest(pngs, "Tauri app icon")
            });
            out.extend(found);
        }
    }

    /// The builder config's `icon` (`package.json`'s `build.icon`, or
    /// `electron-builder.json`), else `build/icon.png` / `.svg`.
    fn electron(&self, out: &mut Vec<Candidate>) {
        let configured = [
            ("package.json", "/build/icon"),
            ("electron-builder.json", "/icon"),
        ]
        .iter()
        .filter_map(|(file, ptr)| {
            self.read_json(file)?
                .pointer(ptr)?
                .as_str()
                .map(str::to_string)
        })
        // The config may name the icon without its extension.
        .flat_map(|p| [p.clone(), format!("{p}.png"), format!("{p}.svg")]);
        let fixed = ["build/icon.png", "build/icon.svg"].map(String::from);
        out.extend(
            configured
                .chain(fixed)
                .find_map(|p| self.candidate(&p, "Electron app icon")),
        );
    }

    /// Each `*.xcassets/AppIcon.appiconset`'s widest PNG.
    fn xcode(&self, out: &mut Vec<Candidate>) {
        for assets in self.dirs.iter().filter(|d| d.ends_with(".xcassets")) {
            let set = format!("{assets}/AppIcon.appiconset");
            let pngs = self
                .children(&set)
                .into_iter()
                .filter(|n| ext(n) == "png")
                .map(|n| format!("{set}/{n}"));
            out.extend(self.widest(pngs, "Xcode app icon"));
        }
    }

    /// Android's `mipmap-*/ic_launcher.png` (or `_round`), densest first. The
    /// adaptive icon's `_foreground` layer is padded artwork, not the icon.
    fn android(&self, out: &mut Vec<Candidate>) {
        for res in ["app/src/main/res", "android/app/src/main/res"] {
            let pngs = self
                .children(res)
                .into_iter()
                .filter(|d| d.starts_with("mipmap-"))
                .flat_map(|d| {
                    ["ic_launcher.png", "ic_launcher_round.png"].map(|n| format!("{res}/{d}/{n}"))
                });
            out.extend(self.widest(pngs, "Android launcher icon"));
        }
    }

    /// `flutter_launcher_icons`' `image_path`, in its own file or in
    /// `pubspec.yaml`.
    fn flutter(&self, out: &mut Vec<Candidate>) {
        for file in ["flutter_launcher_icons.yaml", "pubspec.yaml"] {
            let Some(text) = self.read_text(file) else {
                continue;
            };
            let path = text.lines().find_map(|line| {
                let (key, value) = line.trim().split_once(':')?;
                key.starts_with("image_path")
                    .then(|| value.trim().trim_matches(['"', '\'']).to_string())
            });
            if let Some(c) = path.and_then(|p| self.candidate(&p, "Flutter launcher icon")) {
                out.push(c);
                return;
            }
        }
    }

    // ── 2. What a web app declares ───────────────────────────────────────

    /// Each web manifest's largest icon. `icons` is a list of
    /// `{ src, sizes }`, or a browser extension's `{ "128": src }`.
    fn manifests(&self, out: &mut Vec<Candidate>) {
        let manifests = self.files.iter().filter(|f| {
            let name = name_of(f);
            name == "manifest.json" || name.ends_with(".webmanifest")
        });
        for file in manifests {
            let Some(icons) = self.read_json(file).and_then(|j| j.get("icons").cloned()) else {
                continue;
            };
            let mut listed: Vec<(u32, String)> = match &icons {
                serde_json::Value::Array(list) => list
                    .iter()
                    .filter_map(|i| {
                        let src = i.get("src")?.as_str()?;
                        let sizes = i.get("sizes").and_then(|s| s.as_str()).unwrap_or("");
                        Some((largest_size(sizes), src.to_string()))
                    })
                    .collect(),
                serde_json::Value::Object(map) => map
                    .iter()
                    .filter_map(|(size, src)| Some((size.parse().ok()?, src.as_str()?.to_string())))
                    .collect(),
                _ => continue,
            };
            // Largest first; stable, so the manifest's order breaks a tie. A
            // `src` resolves beside the manifest or, root-absolute, in
            // `public/` — where a manifest under `src/` points when `/` is
            // served from there.
            listed.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
            let base = parent_of(file);
            out.extend(listed.iter().find_map(|(_, src)| {
                [join(base, src), join("public", src)]
                    .iter()
                    .find_map(|p| self.candidate(p, "Web manifest icon"))
            }));
        }
    }

    /// Next.js's file-convention icon.
    fn next_app(&self, out: &mut Vec<Candidate>) {
        out.extend(
            self.files
                .iter()
                .filter(|f| matches!(parent_of(f), "app" | "src/app") && stem_of(f) == "icon")
                .filter_map(|f| self.candidate(f, "Next.js app icon")),
        );
    }

    /// The first `<link rel="…icon…" href>` that resolves, beside the page or
    /// in `public/` (where Vite serves `/`).
    fn index_html(&self, out: &mut Vec<Candidate>) {
        for page in ["index.html", "public/index.html", "src/index.html"] {
            let Some(html) = self.read_text(page) else {
                continue;
            };
            let base = parent_of(page);
            let found = link_icons(&html).into_iter().find_map(|href| {
                [join(base, &href), join("public", &href)]
                    .iter()
                    .find_map(|p| self.candidate(p, "index.html icon"))
            });
            out.extend(found);
        }
    }

    // ── 3. Favicons ──────────────────────────────────────────────────────

    fn favicons(&self, out: &mut Vec<Candidate>) {
        for (names, source) in [
            (&["favicon.svg", "favicon.png"][..], "Favicon"),
            (&["apple-touch-icon.png"][..], "Apple touch icon"),
            (&["favicon.ico"][..], "Favicon"),
        ] {
            out.extend(
                self.files_named(names)
                    .filter_map(|f| self.candidate(f, source)),
            );
        }
    }

    // ── 4. Logos ─────────────────────────────────────────────────────────

    fn logos(&self, out: &mut Vec<Candidate>) {
        out.extend(
            self.files
                .iter()
                .filter(|f| {
                    stem_of(f).eq_ignore_ascii_case("logo")
                        && matches!(
                            name_of(parent_of(f)),
                            "public" | "assets" | "static" | "docs"
                        )
                })
                .filter_map(|f| self.candidate(f, "Logo")),
        );
    }

    /// The first local image the README shows.
    fn readme(&self, out: &mut Vec<Candidate>) {
        let Some(readme) = self
            .files
            .iter()
            .find(|f| !f.contains('/') && f.eq_ignore_ascii_case("readme.md"))
        else {
            return;
        };
        let Some(text) = self.read_text(readme) else {
            return;
        };
        out.extend(
            readme_images(&text)
                .into_iter()
                .find_map(|src| self.candidate(&src, "README image")),
        );
    }
}

// ── Path and markup helpers ─────────────────────────────────────────────

fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn stem_of(path: &str) -> &str {
    let name = name_of(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

fn ext(path: &str) -> String {
    name_of(path)
        .rsplit_once('.')
        .map_or(String::new(), |(_, e)| e.to_ascii_lowercase())
}

fn join(base: &str, rel: &str) -> String {
    if base.is_empty() {
        rel.to_string()
    } else {
        format!("{base}/{rel}")
    }
}

/// A reference from a config or a page as a repo-relative path: no scheme, no
/// query or fragment, no leading `/` or `./`, no `..` out of the tree.
/// `None` for a URL or an empty reference.
fn clean(reference: &str) -> Option<String> {
    let r = reference.trim();
    if r.contains("://") || r.starts_with("data:") || r.starts_with("//") {
        return None;
    }
    let r = r.split(['?', '#']).next().unwrap_or("");
    let mut parts: Vec<&str> = Vec::new();
    for part in r.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The largest edge in a manifest `sizes` ("192x192 512x512"); `any` (a
/// vector icon) beats every bitmap.
fn largest_size(sizes: &str) -> u32 {
    sizes
        .split_whitespace()
        .filter_map(|s| {
            if s.eq_ignore_ascii_case("any") {
                return Some(u32::MAX);
            }
            s.split(['x', 'X']).next()?.parse().ok()
        })
        .max()
        .unwrap_or(0)
}

/// The `href`s of the page's icon links, in document order.
fn link_icons(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<link") {
        let start = from + at;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &html[start..end];
        if attr(tag, "rel").is_some_and(|r| r.to_ascii_lowercase().contains("icon")) {
            out.extend(attr(tag, "href"));
        }
        from = end;
    }
    out
}

/// The value of `name="…"` (or `'…'`) in a tag.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find(name) {
        let start = from + at;
        from = start + name.len();
        // A whole attribute name, not the tail of another (`data-href`).
        let before = lower[..start].chars().next_back();
        if before.is_some_and(|c| !c.is_ascii_whitespace()) {
            continue;
        }
        let Some(rest) = tag[from..].trim_start().strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let value = &rest[1..];
        return value.find(quote).map(|e| value[..e].to_string());
    }
    None
}

/// The image sources a README shows, in order: Markdown `![alt](src)` and
/// HTML `<img src>`.
fn readme_images(text: &str) -> Vec<String> {
    let mut found: Vec<(usize, String)> = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("![") {
        let start = from + at;
        from = start + 2;
        let Some(close) = text[from..].find("](") else {
            break;
        };
        let src_at = from + close + 2;
        if let Some(end) = text[src_at..].find(')') {
            // `![alt](src "title")`: the source is the first word.
            let src = text[src_at..src_at + end].split_whitespace().next();
            found.extend(src.map(|s| (start, s.trim_matches(['<', '>']).to_string())));
        }
    }
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<img") {
        let start = from + at;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        found.extend(attr(&text[start..end], "src").map(|s| (start, s)));
        from = end;
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, src)| src).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_detect::test_support::fixture;

    /// A PNG header claiming `width`: enough for the width read and the
    /// image sniff.
    fn png(width: u32) -> Vec<u8> {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        b.extend(b"IHDR");
        b.extend(width.to_be_bytes());
        b.extend(width.to_be_bytes());
        b
    }

    fn with_pngs(dir: &tempfile::TempDir, files: &[(&str, u32)]) {
        for (path, width) in files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, png(*width)).unwrap();
        }
    }

    fn paths(repo: &Path) -> Vec<String> {
        scan(repo).into_iter().map(|c| c.path).collect()
    }

    #[test]
    fn an_empty_or_missing_repo_finds_nothing() {
        let dir = fixture(&[("README.md", "# x")]);
        assert!(scan(dir.path()).is_empty());
        assert!(scan(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn the_app_icon_outranks_web_icons_and_logos() {
        let dir = fixture(&[
            (
                "src-tauri/tauri.conf.json",
                r#"{"bundle":{"icon":["icons/32x32.png","icons/128x128.png","icons/icon.icns"]}}"#,
            ),
            ("public/favicon.svg", "<svg/>"),
            ("public/logo.svg", "<svg/>"),
            (
                "index.html",
                r#"<link rel="icon" type="image/svg+xml" href="/vite.svg" />"#,
            ),
            ("public/vite.svg", "<svg/>"),
        ]);
        with_pngs(
            &dir,
            &[
                ("src-tauri/icons/32x32.png", 32),
                ("src-tauri/icons/128x128.png", 128),
            ],
        );
        let found = scan(dir.path());
        assert_eq!(found[0].path, "src-tauri/icons/128x128.png");
        assert_eq!(found[0].width, Some(128));
        assert_eq!(found[0].source, "Tauri app icon");
        assert_eq!(
            found
                .iter()
                .skip(1)
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>(),
            ["public/vite.svg", "public/favicon.svg", "public/logo.svg"]
        );
    }

    #[test]
    fn a_tauri_app_without_a_listed_icon_falls_back_to_its_widest_png() {
        let dir = fixture(&[("src-tauri/tauri.conf.json", "{}")]);
        with_pngs(
            &dir,
            &[
                ("src-tauri/icons/32x32.png", 32),
                ("src-tauri/icons/icon.png", 512),
            ],
        );
        assert_eq!(paths(dir.path()), ["src-tauri/icons/icon.png"]);
    }

    #[test]
    fn the_manifest_offers_its_largest_icon() {
        let dir = fixture(&[(
            "public/manifest.json",
            r#"{"icons":[{"src":"/icon-192.png","sizes":"192x192"},{"src":"icon-512.png","sizes":"512x512"}]}"#,
        )]);
        with_pngs(
            &dir,
            &[("public/icon-192.png", 192), ("public/icon-512.png", 512)],
        );
        assert_eq!(paths(dir.path()), ["public/icon-512.png"]);
    }

    #[test]
    fn next_and_xcode_and_android_icons_are_found_by_convention() {
        let dir = fixture(&[("app/icon.svg", "<svg/>")]);
        with_pngs(
            &dir,
            &[
                ("Mac/Assets.xcassets/AppIcon.appiconset/16.png", 16),
                ("Mac/Assets.xcassets/AppIcon.appiconset/1024.png", 1024),
                ("app/src/main/res/mipmap-mdpi/ic_launcher.png", 48),
                ("app/src/main/res/mipmap-xxxhdpi/ic_launcher.png", 192),
                (
                    "app/src/main/res/mipmap-xxxhdpi/ic_launcher_foreground.png",
                    432,
                ),
            ],
        );
        assert_eq!(
            paths(dir.path()),
            [
                "Mac/Assets.xcassets/AppIcon.appiconset/1024.png",
                "app/src/main/res/mipmap-xxxhdpi/ic_launcher.png",
                "app/icon.svg",
            ]
        );
    }

    #[test]
    fn an_ico_is_last_and_dependencies_are_never_read() {
        let dir = fixture(&[
            ("favicon.ico", "\0\0\x01\0"),
            ("node_modules/pkg/favicon.svg", "<svg/>"),
            ("dist/favicon.svg", "<svg/>"),
            ("docs/logo.png", ""),
            (
                "README.md",
                "Intro ![shot](https://x.test/a.png) and ![logo](./docs/logo.png)",
            ),
        ]);
        assert_eq!(paths(dir.path()), ["docs/logo.png", "favicon.ico"]);
    }

    #[test]
    fn the_readme_image_is_the_last_resort_and_stays_inside_the_repo() {
        let dir = fixture(&[
            (
                "README.md",
                r#"<p><img src="../outside.png"></p> ![](art/banner.svg "Banner")"#,
            ),
            ("art/banner.svg", "<svg/>"),
        ]);
        assert_eq!(paths(dir.path()), ["art/banner.svg"]);
    }

    #[test]
    fn a_secondary_repo_ranks_after_the_primary_under_its_folder_name() {
        let primary = fixture(&[("public/favicon.svg", "<svg/>")]);
        let secondary = fixture(&[("static/logo.svg", "<svg/>")]);
        let found = scan_project(&[primary.path().to_path_buf(), secondary.path().to_path_buf()]);
        let name = secondary.path().file_name().unwrap().to_string_lossy();
        assert_eq!(found[0].path, "public/favicon.svg");
        assert_eq!(found[1].path, format!("{name}/static/logo.svg"));
    }

    #[test]
    fn references_are_cleaned_to_repo_paths() {
        assert_eq!(clean("/icons/a.png?v=2").as_deref(), Some("icons/a.png"));
        assert_eq!(clean("./a/../b.svg#x").as_deref(), Some("b.svg"));
        assert_eq!(clean("../up.png"), None);
        assert_eq!(clean("https://x.test/a.png"), None);
        assert_eq!(largest_size("48x48 any"), u32::MAX);
        assert_eq!(
            link_icons(
                r#"<link data-rel="x" rel="shortcut icon" href='/f.png'><link rel=stylesheet href="a.css">"#
            ),
            ["/f.png"]
        );
    }
}
