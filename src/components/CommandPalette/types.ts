import type { IconName } from "@/components/Icon";
import type { DotStatus } from "@/components/TitleBar/WorkspaceStatus/derive";

/** The drill-in pages a row can open. `root` is the search over everything;
 *  the rest list one kind of thing to pick (a project to start an agent in,
 *  a host to switch to, an accent). */
export type PageId =
  | "root"
  | "new-agent"
  | "workflow"
  | "environment"
  | "settings"
  | "editor"
  | "accent";

/** What stands at the front of a row: a glyph on a tile, an agent's status
 *  dot, or a colour swatch. */
export type Leading =
  | { kind: "icon"; name: IconName }
  | { kind: "status"; status: DotStatus }
  | { kind: "swatch"; color: string };

export interface PaletteItem {
  /** Stable across renders: the recents list and the active row key on it. */
  id: string;
  label: string;
  /** Second phrase on the row, dimmer: what it does, or where it lives. */
  hint?: string;
  /** Right-hand detail when the row has no chord to show: a status word. */
  meta?: string;
  leading: Leading;
  /** The group the row sorts into. */
  section: string;
  /** A keymap id whose chord to show on the right — and the proof the row
   *  and the chord do the same thing. */
  shortcut?: string;
  /** Extra words the row answers to, separated by two spaces. */
  keywords?: string;
  /** Why the row can't run right now. Shown dimmed with the reason. */
  disabled?: string | null;
  /** A drill-in: Enter opens this page instead of acting. */
  page?: PageId;
  /** What Enter does. Absent on a page row. */
  run?: () => void;
  /** Remembered in the recents list when run. Commands yes; agents, runs and
   *  projects no — those are ranked by what they are doing. */
  remember?: boolean;
}

/** One row as shown: the item and, while searching, where the query landed
 *  on its label. */
export interface Row {
  item: PaletteItem;
  indices: number[];
}

export interface Section {
  label: string;
  rows: Row[];
}

/** How a page introduces itself: the chip beside the input and the prompt in it. */
export interface PageSpec {
  title: string;
  placeholder: string;
}

export const PAGES: Record<PageId, PageSpec> = {
  root: { title: "", placeholder: "Search agents, projects, commands…" },
  "new-agent": { title: "New agent", placeholder: "Which project?" },
  workflow: { title: "Run workflow", placeholder: "Which project?" },
  environment: { title: "Environment", placeholder: "Switch to…" },
  settings: { title: "Settings", placeholder: "Which section?" },
  editor: { title: "Open in", placeholder: "Which editor?" },
  accent: { title: "Accent", placeholder: "Pick a colour" },
};
