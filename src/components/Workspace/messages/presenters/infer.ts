// What a tool call is when no presenter knows the tool: a kind read off the
// tool's name, and the file, command or query read off its input.
//
// Every provider names its tools its own way — Claude `Read`, Cursor `read`,
// Antigravity `view_file`, Codex `exec_command` — and the names a presenter
// claims resolve first (presenters/index.ts). This is the floor under the
// rest: a run of tools nobody registered still reads "changed 2 files · ran 1
// command" rather than "used 3 tools", and its live line still says what is
// happening rather than which tool is doing it. Pure; no rendering.

import { basename } from "@/util/format";
import type { StepFacts, StepKind, ToolCall } from "./types";
import { commandHead, prettyToolName } from "./util";

/** The words of a tool name, lower-cased: `write_to_file`, `writeToFile` and
 *  `mcp__fs__write-file` all give `["write", "to", "file"]`. */
export function nameWords(name: string): string[] {
  return prettyToolName(name)
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean);
}

/** The words that place a tool in a family. Checked in this order: a plan
 *  tool often "writes" (`todo_write`), a change tool may "search" (`search_replace`),
 *  a search tool may "read" (`read_search_results`). Only words that name
 *  the action — a tool called `get_user` stays `other`. */
const FAMILIES: [StepKind, ReadonlySet<string>][] = [
  ["plan", new Set(["todo", "todos", "plan", "checklist"])],
  [
    "change",
    new Set([
      "write",
      "edit",
      "patch",
      "replace",
      "create",
      "delete",
      "remove",
      "rename",
      "move",
      "mkdir",
      "touch",
      "append",
      "multiedit",
    ]),
  ],
  [
    "run",
    new Set([
      "bash",
      "shell",
      "exec",
      "execute",
      "command",
      "cmd",
      "run",
      "terminal",
      "powershell",
    ]),
  ],
  [
    "search",
    new Set([
      "grep",
      "search",
      "find",
      "glob",
      "list",
      "ls",
      "dir",
      "directory",
      "locate",
      "codebase",
    ]),
  ],
  ["read", new Set(["read", "view", "cat", "open", "fetch"])],
];

export function familyOf(name: string): StepKind {
  const words = nameWords(name);
  for (const [kind, set] of FAMILIES) if (words.some((w) => set.has(w))) return kind;
  return "other";
}

/** The input's fields, when it has any. */
function fieldsOf(input: unknown): Record<string, unknown> | null {
  return input && typeof input === "object" && !Array.isArray(input)
    ? (input as Record<string, unknown>)
    : null;
}

/** A string value as a person wrote it. Antigravity stores argument values
 *  JSON-encoded (`'"/x"'` for the path `/x`), so a quoted value is unquoted. */
function asText(value: unknown): string {
  if (Array.isArray(value)) return value.every((v) => typeof v === "string") ? value.join(" ") : "";
  if (typeof value !== "string") return "";
  if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) {
    try {
      const parsed: unknown = JSON.parse(value);
      return typeof parsed === "string" ? parsed : value;
    } catch {
      return value;
    }
  }
  return value;
}

/** The first non-empty text among the input's fields whose key matches. */
function field(input: unknown, key: RegExp): string {
  const fields = fieldsOf(input);
  if (!fields) return "";
  for (const [k, v] of Object.entries(fields)) {
    if (!key.test(k)) continue;
    const text = asText(v);
    if (text) return text;
  }
  return "";
}

const PATH_KEY = /path|file|dir/i;
const COMMAND_KEY = /^(command|cmd|command_?line|script|code)$/i;
const QUERY_KEY = /query|pattern|search|regex|term/i;

/** The file or directory a call names, under whatever key: `file_path`,
 *  `path`, `TargetFile`, `AbsolutePath`, `DirectoryPath`, `filename`. */
export function pathOf(input: unknown): string {
  return field(input, PATH_KEY);
}

/** The command a call runs: a bare string or argv, or a `command`-like field. */
export function commandOf(input: unknown): string {
  if (typeof input === "string" || Array.isArray(input)) return asText(input);
  return field(input, COMMAND_KEY);
}

/** What a call searches for, under whatever key. */
export function queryOf(input: unknown): string {
  return typeof input === "string" ? input : field(input, QUERY_KEY);
}

export function inferFacts(call: ToolCall): StepFacts {
  const label = prettyToolName(call.name);
  const kind = familyOf(call.name);
  switch (kind) {
    case "change":
    case "read": {
      const path = pathOf(call.input);
      // A change or read that names no file can't be counted as one.
      return path ? { kind, path } : { kind: "other", subject: label };
    }
    case "run":
      return { kind, subject: commandHead(commandOf(call.input)) || label };
    case "search":
      return { kind, subject: queryOf(call.input) || pathOf(call.input) };
    case "plan":
      return { kind };
    default:
      return { kind, subject: label };
  }
}

export function inferActivity(call: ToolCall): string {
  const label = prettyToolName(call.name);
  switch (familyOf(call.name)) {
    case "change": {
      const path = pathOf(call.input);
      return path ? `Editing ${basename(path)}` : `Running ${label}`;
    }
    case "read": {
      const path = pathOf(call.input);
      return path ? `Reading ${basename(path)}` : `Running ${label}`;
    }
    case "run": {
      const head = commandHead(commandOf(call.input));
      return head ? `Running ${head}` : "Running a command";
    }
    case "search": {
      const what = queryOf(call.input) || pathOf(call.input);
      return what ? `Searching ${what.length > 48 ? `${what.slice(0, 47)}…` : what}` : "Searching";
    }
    case "plan":
      return "Updating the plan";
    default:
      return `Using ${label}`;
  }
}
