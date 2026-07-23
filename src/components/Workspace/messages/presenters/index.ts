import { agentPresenter } from "./Agent";
import { bashPresenter } from "./Bash";
import { codegraphPresenter } from "./Codegraph";
import { defaultPresenter } from "./default";
import { editPresenter } from "./Edit";
import { globPresenter } from "./Glob";
import { grepPresenter } from "./Grep";
import { multiEditPresenter } from "./MultiEdit";
import { readPresenter } from "./Read";
import { taskCreatePresenter } from "./TaskCreate";
import type { ToolPresenter } from "./types";
import { writePresenter } from "./Write";

export type { ToolCall, ToolPresenter, ToolResult } from "./types";

export const PRESENTERS: Record<string, ToolPresenter> = {
  Agent: agentPresenter,
  Bash: bashPresenter,
  // Codex and Cursor name their shell tool `shell`; same UI as Claude's `Bash`.
  shell: bashPresenter,
  Read: readPresenter,
  Edit: editPresenter,
  MultiEdit: multiEditPresenter,
  Write: writePresenter,
  Grep: grepPresenter,
  Glob: globPresenter,
  TaskCreate: taskCreatePresenter,
};

// Look up on the lowercased tool name so adapters that report the same tool
// in a different case (e.g. cursor's `read`/`glob`) match without extra
// entries. Tools with a genuinely different name go in PRESENTERS directly
// (e.g. `shell`).
const BY_KEY: Record<string, ToolPresenter> = Object.fromEntries(
  Object.entries(PRESENTERS).map(([name, p]) => [name.toLowerCase(), p]),
);

// Prefix matches for tool families that share one presenter — MCP servers
// expose many tools under a `mcp__<server>__*` namespace, so a single presenter
// covers the whole family (e.g. every `mcp__codegraph__*` tool). Checked only
// after an exact BY_KEY miss.
const BY_PREFIX: { prefix: string; presenter: ToolPresenter }[] = [
  { prefix: "mcp__codegraph__", presenter: codegraphPresenter },
];

export function getPresenter(toolName: string): ToolPresenter {
  const key = toolName.toLowerCase();
  const exact = BY_KEY[key];
  if (exact) return exact;
  const prefixed = BY_PREFIX.find((entry) => key.startsWith(entry.prefix));
  return prefixed?.presenter ?? defaultPresenter;
}
