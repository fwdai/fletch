// The extension contract.
//
// This is the entire surface the core exposes for extending the app. An
// extension is a self-contained unit that contributes UI (and, later, more) to
// the core through these typed slots. The core knows the *shape* of an
// extension — never which specific extensions exist.
//
// Extensions live under the repo-root `extensions/` directory and are
// discovered automatically at build time (see ./registry.ts). Whether a given
// extension is public (committed) or private (its own repo, gitignored, cloned
// in) is purely a matter of where its source lives — not a concept the core
// models. See extensions/README.md.

import type { ComponentType } from "react";
import type { IconName } from "../components/Icon";

/** A settings pane contributed by an extension, mounted by SettingsScreen
 *  alongside the built-in panes. `id` is namespaced `ext:*` so it slots into
 *  `SettingsSection` without the core enumerating extensions. */
export interface ExtensionSettingsPane {
  id: `ext:${string}`;
  label: string;
  icon: IconName;
  Component: ComponentType;
}

/** What a single extension exports (as `export const extension: Extension`)
 *  from its `index.ts`. Every contribution field is optional so an extension
 *  opts into only the slots it needs; new slots are added here as new seams
 *  appear (nav items, commands, panels, …). */
export interface Extension {
  /** Stable, unique identifier for the extension, e.g. "sync". */
  id: string;
  settingsPanes?: ExtensionSettingsPane[];
}
