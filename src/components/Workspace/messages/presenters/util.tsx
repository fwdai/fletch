import type { ReactNode } from "react";

/** Mono-styled block used by presenter expanded views. `label` is a short
 *  prefix (e.g. "$" for a shell command, "file" for a path). */
export function ToolBlock({
  label,
  isError,
  children,
}: {
  label?: string;
  isError?: boolean;
  children: ReactNode;
}) {
  return (
    <pre
      style={{
        margin: 0,
        padding: "4px 0",
        fontFamily: "var(--font-mono)",
        fontSize: 11.5,
        color: isError ? "var(--danger)" : "var(--fg-2)",
        whiteSpace: "pre-wrap",
        wordBreak: "break-word",
      }}
    >
      {label && (
        <span style={{ color: "var(--fg-3)", marginRight: 8 }}>{label}</span>
      )}
      {children}
    </pre>
  );
}

/** Flatten a tool_result.content payload to text. Accepts strings,
 *  anthropic content-block arrays, or arbitrary JSON. */
export function renderToolResult(content: unknown): string {
  if (content == null) return "";
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((block) => {
        if (block && typeof block === "object" && "text" in block) {
          return String((block as { text: unknown }).text ?? "");
        }
        return typeof block === "string" ? block : JSON.stringify(block);
      })
      .join("\n");
  }
  return JSON.stringify(content, null, 2);
}

/** Best-effort string view of tool call input. Used by the default presenter
 *  and as a fallback inside specialized ones. */
export function stringifyInput(input: unknown, indent = 0): string {
  if (input == null) return "";
  if (typeof input === "string") return input;
  try {
    return JSON.stringify(input, null, indent);
  } catch {
    return "";
  }
}

/** Compact "+X −Y" line-count badge for file-editing tools. Renders nothing
 *  when there is no net change. Colors mirror the git panel's add/rem tokens. */
export function DiffCount({
  additions,
  deletions,
}: {
  additions: number;
  deletions: number;
}) {
  if (additions === 0 && deletions === 0) return null;
  return (
    <span style={{ fontFamily: "var(--font-mono)", marginLeft: 6 }}>
      {additions > 0 && (
        <span style={{ color: "var(--success)" }}>+{additions}</span>
      )}
      {additions > 0 && deletions > 0 && " "}
      {deletions > 0 && (
        <span style={{ color: "var(--danger)" }}>−{deletions}</span>
      )}
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
  return (
    <span style={{ color: "var(--fg-3)", marginLeft: 6 }}>{children}</span>
  );
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

/** Truncate at the first newline, with an ellipsis if there's more. */
export function firstLineOf(text: string, max = 120): string {
  const nl = text.indexOf("\n");
  const head = (nl === -1 ? text : text.slice(0, nl)).trim();
  if (head.length <= max && nl === -1) return head;
  return head.length > max ? `${head.slice(0, max - 1)}…` : `${head}…`;
}
