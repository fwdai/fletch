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
  /** Hint shown in the empty field when there's no detected value to
   *  suggest — only the fallback rows carry one. */
  placeholder?: string;
  /** Where `value` comes from: repo detection (default) or an explicit
   *  project setting layered on top. The Run panel maps project settings
   *  onto detected rows so agent-level edits compare against the value the
   *  agent actually inherits. */
  origin?: "detected" | "project";
}

/** Backend group key → display label. */
const GROUP_LABEL: Record<string, string> = {
  environment: "Environment",
  scripts: "Scripts",
  server: "Server",
};

/** The fields every project can configure, even when the repo yields no
 *  detection. Ids/groups mirror the backend schema so an entered value
 *  persists under the same key a detector would have filled — and is picked
 *  up by `read_run_commands` at run time. Rendered empty (no default,
 *  `source: ""`) with a hint so the user can type their own command. */
const FALLBACK_ROWS: readonly SetupRow[] = [
  row("version", "environment", "Version", "Language / runtime version"),
  row("install", "environment", "Install", "Command to install dependencies"),
  row("dev", "scripts", "Dev", "Command to start the dev server"),
  row("test", "scripts", "Test", "Command to run the tests"),
  row("build", "scripts", "Build", "Command to build the project"),
  row("port", "server", "Port", "e.g. 3000"),
];

function row(id: string, group: string, key: string, placeholder: string): SetupRow {
  return { id, group: GROUP_LABEL[group], key, value: "", source: "", placeholder };
}

/** Rows a surface should render: the detected ones when the repo yielded
 *  any, else the empty fallback set so run config is always editable rather
 *  than hidden behind a "nothing detected" state. */
export function rowsOrFallback(rows: SetupRow[]): SetupRow[] {
  return rows.length > 0 ? rows : [...FALLBACK_ROWS];
}

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
