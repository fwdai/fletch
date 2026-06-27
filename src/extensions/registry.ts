// Build-time extension discovery.
//
// Every `extensions/<name>/index.ts` present on disk at build time is loaded
// and its `extension` export collected. There is no manifest and no enable
// flag: an extension is active iff its folder exists in the build. A fresh
// public clone / CI only has the committed (public) extensions, so private
// extensions — which live in their own repos and are gitignored when cloned in
// — are simply absent and never reach the bundle. This is what keeps
// unpublished work out of the open-source build, with no special-casing.
//
// Adding an extension costs nothing here: drop in a folder, rebuild.

import type { Extension, ExtensionSettingsPane } from "./types";

// Eager so contributions are available synchronously at module load. The glob
// pattern is repo-root-relative to this file: <root>/extensions/<name>/index.ts.
const modules = import.meta.glob<{ extension: Extension }>("../../extensions/*/index.ts", {
  eager: true,
});

/** Every discovered extension, in arbitrary (path) order. */
export const extensions: Extension[] = Object.values(modules)
  .map((m) => m.extension)
  .filter(Boolean);

/** Flattened contribution slots, ready for the core's mount sites. */
export const settingsPanes: ExtensionSettingsPane[] = extensions.flatMap(
  (e) => e.settingsPanes ?? [],
);
