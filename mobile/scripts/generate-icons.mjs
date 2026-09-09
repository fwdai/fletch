import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const output = join(root, ".tmp", "icons");
const icons = join(root, "src-tauri", "icons");
mkdirSync(output, { recursive: true });
try {
  execFileSync("bun", ["run", "tauri", "icon", "app-icon.png", "--output", output], {
    cwd: root,
    stdio: "inherit",
  });
  const ios = join(output, "ios");
  execFileSync(
    "swift",
    [
      "-module-cache-path",
      join(root, ".tmp", "swift-module-cache"),
      join(root, "scripts", "opaque-icons.swift"),
      ...readdirSync(ios)
        .filter((name) => name.endsWith(".png"))
        .map((name) => join(ios, name)),
    ],
    { stdio: "inherit" },
  );
  // Keep only the assets used by this iOS companion and its desktop preview.
  for (const name of ["ios", "32x32.png", "128x128.png", "128x128@2x.png", "icon.icns"]) {
    cpSync(join(output, name), join(icons, name), { recursive: true });
  }
  // ios init copies icons into an ignored, machine-local Xcode asset catalog.
  // Refresh its PNGs too, preserving the generated Contents.json.
  const syncCatalogs = (dir) => {
    if (!existsSync(dir)) return;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const path = join(dir, entry.name);
      if (entry.name === "AppIcon.appiconset") {
        cpSync(join(icons, "ios"), path, { recursive: true });
        console.log(`Updated ${path}`);
      } else if (!["build", "Pods", "DerivedData"].includes(entry.name)) {
        syncCatalogs(path);
      }
    }
  };
  syncCatalogs(join(root, "src-tauri", "gen", "apple"));
} finally {
  rmSync(output, { recursive: true, force: true });
}
