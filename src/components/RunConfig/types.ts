import type { DetectedRow } from "@/api";

/** A single run-config row as rendered by the editor: the detected value
 *  (`value`) is the default/placeholder; a persisted override layers on top.
 *  `group` holds the display label (already mapped from the backend key). */
export interface SetupRow {
  id: string;
  group: string;
  key: string;
  value: string; // inferred / default
  source: string; // e.g. "scripts.dev", "vite.config.ts"
}

/** Backend group key → display label. Consumed only by `toSetupRows`. */
const GROUP_LABEL: Record<string, string> = {
  environment: "Environment",
  scripts: "Scripts",
  server: "Server",
};

/** Ecosystem key → display label. */
export const ECOSYSTEM_LABEL: Record<string, string> = {
  node: "Node",
  python: "Python",
  ruby: "Ruby",
  rust: "Rust",
  go: "Go",
};

/** Map detected rows from the backend to the display rows the editor renders,
 *  applying the group label. Shared by every surface that shows run config. */
export function toSetupRows(rows: DetectedRow[]): SetupRow[] {
  return rows.map((r) => ({
    id: r.id,
    group: GROUP_LABEL[r.group] ?? r.group,
    key: r.key,
    value: r.value,
    source: r.source,
  }));
}
