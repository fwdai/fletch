// Presentation for a tool call: an icon, a tint and a one-line argument
// summary. Keyed by the tool names the agents actually emit, with a neutral
// fallback so an unknown tool still renders.

import type { IconName } from "../components/Icon";

export const TOOL_ICON: Record<string, IconName> = {
  Bash: "terminal",
  BashOutput: "terminal",
  Edit: "edit",
  Write: "file",
  Read: "file",
  Grep: "search",
  Glob: "search",
  WebFetch: "fetch",
  WebSearch: "fetch",
  Task: "task",
  TodoWrite: "task",
  exec_command: "terminal",
  shell: "terminal",
  apply_patch: "edit",
};

export const TOOL_HUE: Record<string, string> = {
  Bash: "var(--accent)",
  Edit: "var(--accent)",
  Write: "var(--success)",
  Read: "var(--fg-2)",
  Grep: "var(--info)",
  Glob: "var(--info)",
  WebFetch: "var(--info)",
  Task: "var(--merged)",
  exec_command: "var(--accent)",
  apply_patch: "var(--accent)",
};

const str = (v: unknown) => (typeof v === "string" ? v : undefined);

/** The most informative single field of a tool's input. */
export function toolArg(input: unknown): string {
  if (typeof input === "string") return input;
  if (!input || typeof input !== "object") return "";
  const o = input as Record<string, unknown>;
  const candidate =
    str(o.command) ??
    str(o.cmd) ??
    str(o.file_path) ??
    str(o.path) ??
    str(o.pattern) ??
    str(o.url) ??
    str(o.description) ??
    str(o.prompt);
  if (candidate) return candidate;
  const keys = Object.keys(o);
  return keys.length ? `${keys[0]}: ${JSON.stringify(o[keys[0]]).slice(0, 60)}` : "";
}

/** Flatten a tool_result body (string, or Anthropic content blocks) to text. */
export function resultText(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((b) =>
        b && typeof b === "object" && "text" in b ? String((b as { text: unknown }).text) : "",
      )
      .join("")
      .trim();
  }
  if (content && typeof content === "object") return JSON.stringify(content);
  return "";
}

/** First line of a result, for the collapsed row's right-hand meta slot. */
export function resultSummary(text: string): string {
  const line = text.trim().split("\n")[0] ?? "";
  return line.length > 42 ? `${line.slice(0, 41)}…` : line;
}
