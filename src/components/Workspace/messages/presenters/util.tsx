import type { ReactNode } from "react";
import { renderToolResult } from "@/util/toolText";

/** Compact "+X −Y" line-count badge for file-editing tools. Renders nothing
 *  when there is no net change. Colors mirror the git panel's add/rem tokens. */
export function DiffCount({ additions, deletions }: { additions: number; deletions: number }) {
  if (additions === 0 && deletions === 0) return null;
  return (
    <span className="diff-count">
      {additions > 0 && <span className="add">+{additions}</span>}
      {additions > 0 && deletions > 0 && " "}
      {deletions > 0 && <span className="del">−{deletions}</span>}
    </span>
  );
}

/** Count the non-empty lines of text in a tool result's content. */
export function countResultLines(result: { content: unknown } | null): number {
  if (!result) return 0;
  const text = renderToolResult(result.content).replace(/\n+$/, "");
  return text === "" ? 0 : text.split("\n").length;
}

/** Muted trailing note for a tool summary, e.g. "20 lines". Renders nothing
 *  when empty. */
export function SummaryNote({ children }: { children: ReactNode }) {
  if (children == null || children === "") return null;
  return <span className="summary-note">{children}</span>;
}

/** Type-narrowing helper: pull a string field from an unknown input bag. */
export function getStringField(input: unknown, field: string): string {
  if (input && typeof input === "object" && field in input) {
    const v = (input as Record<string, unknown>)[field];
    if (typeof v === "string") return v;
  }
  return "";
}

/** Read a shell command from a tool input. Handles every shape adapters
 *  produce: Claude's `Bash` wraps it in a `{ command }` object, while Codex
 *  and Cursor hand over the command as a bare value — either a string or an
 *  argv array like ["bash", "-lc", "…"]. */
export function getCommandField(input: unknown, field = "command"): string {
  if (typeof input === "string") return input;
  if (Array.isArray(input)) return input.map((part) => String(part)).join(" ");
  if (input && typeof input === "object" && field in input) {
    const v = (input as Record<string, unknown>)[field];
    if (typeof v === "string") return v;
    if (Array.isArray(v)) return v.map((part) => String(part)).join(" ");
  }
  return "";
}

/** The part of a shell command worth naming: its first line, without the
 *  `cd <dir> &&` and `VAR=value` preamble agents put in front of the real
 *  command, nor the `bash -lc` wrapper some adapters hand over. */
export function commandHead(cmd: string, max = 60): string {
  let head = cmd.trim();
  const wrapped = /^(?:\/\S*\/)?(?:ba|z)?sh\s+-l?c\s+(['"])([\s\S]*)\1$/.exec(head);
  if (wrapped) head = wrapped[2].trim();
  for (;;) {
    const next = head
      .replace(/^cd\s+(?:"[^"]*"|'[^']*'|\S+)\s*(?:&&|;)\s*/, "")
      .replace(/^[A-Z_][A-Z0-9_]*=(?:"[^"]*"|'[^']*'|\S*)\s+/, "");
    if (next === head) break;
    head = next;
  }
  return firstLineOf(head, max);
}

/** Present participles of the verbs an agent opens its own description of a
 *  command with ("Run the unit tests"). A closed list on purpose: an unknown
 *  first word ("Git status", "Python script") would conjugate into nonsense,
 *  so it misses here and the live line names the command instead. */
const PARTICIPLES: Record<string, string> = {
  add: "Adding",
  apply: "Applying",
  build: "Building",
  bump: "Bumping",
  check: "Checking",
  clean: "Cleaning",
  clone: "Cloning",
  commit: "Committing",
  compare: "Comparing",
  compile: "Compiling",
  confirm: "Confirming",
  copy: "Copying",
  count: "Counting",
  create: "Creating",
  delete: "Deleting",
  diff: "Diffing",
  download: "Downloading",
  execute: "Executing",
  extract: "Extracting",
  fetch: "Fetching",
  find: "Finding",
  format: "Formatting",
  generate: "Generating",
  get: "Getting",
  inspect: "Inspecting",
  install: "Installing",
  kill: "Killing",
  launch: "Launching",
  lint: "Linting",
  list: "Listing",
  locate: "Locating",
  look: "Looking",
  make: "Making",
  measure: "Measuring",
  merge: "Merging",
  move: "Moving",
  open: "Opening",
  preview: "Previewing",
  print: "Printing",
  pull: "Pulling",
  push: "Pushing",
  query: "Querying",
  read: "Reading",
  rebase: "Rebasing",
  remove: "Removing",
  rename: "Renaming",
  rerun: "Re-running",
  "re-run": "Re-running",
  restart: "Restarting",
  run: "Running",
  scan: "Scanning",
  search: "Searching",
  set: "Setting",
  show: "Showing",
  stage: "Staging",
  start: "Starting",
  stop: "Stopping",
  sync: "Syncing",
  test: "Testing",
  trace: "Tracing",
  typecheck: "Typechecking",
  update: "Updating",
  validate: "Validating",
  verify: "Verifying",
  view: "Viewing",
  wait: "Waiting",
  watch: "Watching",
  write: "Writing",
};

/** "Run the unit tests" → "Running the unit tests"; null when the first word
 *  isn't a verb we know (see PARTICIPLES). */
export function progressive(description: string): string | null {
  const m = /^(\S+)(.*)$/s.exec(firstLineOf(description.trim(), 80));
  if (!m) return null;
  const ing = PARTICIPLES[m[1].toLowerCase()];
  return ing ? `${ing}${m[2]}` : null;
}

/** A tool's name as a person would say it: MCP's `mcp__server__tool` drops its
 *  namespace, and snake_case reads as words (`update_plan` → "update plan"). */
export function prettyToolName(name: string): string {
  const bare = name.replace(/^mcp__[^_]+(?:_[^_]+)*?__/, "").replace(/^[\w-]+\./, "");
  return bare.includes("_") ? bare.replace(/_+/g, " ").trim() : bare;
}

/** One-line, punctuation-free view of an arbitrary tool input, for tools with
 *  no dedicated presenter. Dumping `JSON.stringify` there spent the row's whole
 *  width on braces and quotes, so instead: a lone field shows just its value
 *  (`{query: "…"}` reads as the query), several show `key value` pairs. The
 *  expanded view still carries the exact JSON. */
export function describeInput(input: unknown, max = 120): string {
  if (input == null) return "";
  if (typeof input !== "object") return firstLineOf(compactValue(input), max);
  if (Array.isArray(input)) return firstLineOf(compactValue(input), max);
  const entries = compactEntries(input as Record<string, unknown>);
  if (entries.length === 0) return "";
  if (entries.length === 1) return firstLineOf(entries[0][1], max);
  return firstLineOf(entries.map(([k, v]) => `${k} ${v}`).join(" · "), max);
}

/** Compact every field, keeping only those that render to something. Emptiness
 *  is judged *after* compacting, so a field that contributes nothing but its
 *  own key — `null`, `""`, `[]`, or an object whose fields are all empty —
 *  drops out at any depth. Note `0` and `false` compact to "0"/"false" and are
 *  kept: they're values the tool was actually called with. */
function compactEntries(obj: Record<string, unknown>): [key: string, text: string][] {
  const out: [string, string][] = [];
  for (const [key, value] of Object.entries(obj)) {
    const text = compactValue(value);
    if (text) out.push([key, text]);
  }
  return out;
}

/** Flatten one value to inline text. Whitespace collapses so a multi-line
 *  string (a prompt, a patch) stays on the summary's single line. */
function compactValue(value: unknown): string {
  if (value == null) return "";
  if (typeof value === "string") return value.replace(/\s+/g, " ").trim();
  if (Array.isArray(value)) return value.map(compactValue).filter(Boolean).join(", ");
  if (typeof value === "object") {
    return compactEntries(value as Record<string, unknown>)
      .map(([key, text]) => `${key} ${text}`)
      .join(" ");
  }
  return String(value);
}

/** Truncate at the first newline, with an ellipsis if there's more. */
export function firstLineOf(text: string, max = 120): string {
  const nl = text.indexOf("\n");
  const head = (nl === -1 ? text : text.slice(0, nl)).trim();
  if (head.length <= max && nl === -1) return head;
  return head.length > max ? `${head.slice(0, max - 1)}…` : `${head}…`;
}
