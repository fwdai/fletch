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

/** Recognize a codegraph MCP tool across every adapter's naming convention.
 *  The same server is spelled differently by each: Claude passes the raw
 *  `mcp__codegraph__codegraph_explore` through; Codex renders it
 *  `codegraph.codegraph_explore` (its reducer joins server+tool with a dot and
 *  normalize.ts strips `mcp__`); others may `_`-join (`codegraph_codegraph_…`)
 *  or drop the server. Rather than chase each literal prefix, match the
 *  distinctive signal common to all: a `codegraph` server token followed by a
 *  separator, optionally behind an `mcp__`. `key` is already lowercased. */
function isCodegraphTool(key: string): boolean {
  return /^codegraph[._]/.test(key.replace(/^mcp__/, ""));
}

// Fuzzy matches for tool families that share one presenter — checked only after
// an exact BY_KEY miss. MCP servers expose many tools under one namespace, and
// adapters spell that namespace inconsistently (see isCodegraphTool), so each
// entry is a predicate over the lowercased name rather than a literal prefix.
const FUZZY: { match: (key: string) => boolean; presenter: ToolPresenter }[] = [
  { match: isCodegraphTool, presenter: codegraphPresenter },
];

export function getPresenter(toolName: string): ToolPresenter {
  const key = toolName.toLowerCase();
  return BY_KEY[key] ?? FUZZY.find((entry) => entry.match(key))?.presenter ?? defaultPresenter;
}
