import { agentPresenter } from "./Agent";
import { bashPresenter } from "./Bash";
import { codegraphPresenter } from "./Codegraph";
import { collabPresenter } from "./Collab";
import { defaultPresenter } from "./default";
import { editPresenter } from "./Edit";
import { globPresenter } from "./Glob";
import { grepPresenter } from "./Grep";
import { inferActivity, inferFacts } from "./infer";
import { listPresenter } from "./List";
import { multiEditPresenter } from "./MultiEdit";
import { patchPresenter } from "./Patch";
import { planPresenter } from "./Plan";
import { readPresenter } from "./Read";
import { taskCreatePresenter } from "./TaskCreate";
import type { StepFacts, ToolCall, ToolPresenter, ToolResult } from "./types";
import { prettyToolName } from "./util";
import { webFetchPresenter, webSearchPresenter } from "./Web";
import { writePresenter } from "./Write";

export type { StepFacts, StepKind, ToolCall, ToolPresenter, ToolResult } from "./types";

// Keyed by the name each provider's reducer hands over; the lookup is
// case-insensitive, so Cursor's `read`, OpenCode's `bash` and Pi's `write`
// land on Claude's presenters without entries of their own. Names that differ
// in more than case get an entry here.
export const PRESENTERS: Record<string, ToolPresenter> = {
  Agent: agentPresenter,
  // Cursor's sub-agent tool (and Claude's former name for its own): same input
  // shape — description / subagent_type / prompt — so the same row.
  Task: agentPresenter,
  Bash: bashPresenter,
  // Codex and Cursor name their shell tool `shell`; same UI as Claude's `Bash`.
  shell: bashPresenter,
  // Codex's replayed `exec_command` / `exec` (a bare command string).
  exec_command: bashPresenter,
  exec: bashPresenter,
  Read: readPresenter,
  Edit: editPresenter,
  MultiEdit: multiEditPresenter,
  Write: writePresenter,
  // Codex's `apply_patch` and OpenCode's `patch`: one text, several files.
  apply_patch: patchPresenter,
  patch: patchPresenter,
  Grep: grepPresenter,
  Glob: globPresenter,
  // Directory listings: Claude `LS`, Cursor `ls`, OpenCode `list`, Antigravity `list_dir`.
  LS: listPresenter,
  list: listPresenter,
  list_dir: listPresenter,
  TaskCreate: taskCreatePresenter,
  TodoWrite: planPresenter,
  // Codex's checklist tool; same list, `plan: [{ step, status }]`.
  update_plan: planPresenter,
  WebFetch: webFetchPresenter,
  WebSearch: webSearchPresenter,
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
  // Codex's multi-agent coordination calls: `collab.wait`, `collab.send_input`, …
  { match: (key) => key.startsWith("collab."), presenter: collabPresenter },
];

export function getPresenter(toolName: string): ToolPresenter {
  const key = toolName.toLowerCase();
  return BY_KEY[key] ?? FUZZY.find((entry) => entry.match(key))?.presenter ?? defaultPresenter;
}

/** The name a step summary or live line gives a tool: its presenter's title,
 *  else the raw name made readable. */
export function toolLabel(name: string): string {
  return getPresenter(name).title ?? prettyToolName(name);
}

/** What a call did, for the summary over its run of steps. A tool without a
 *  presenter of its own is placed by its name and input (presenters/infer.ts):
 *  `write_to_file` is a change, `run_command` a run, `grep_search` a search. */
export function stepFacts(call: ToolCall, result: ToolResult | null): StepFacts {
  return getPresenter(call.name).facts?.(call, result) ?? inferFacts(call);
}

/** Present tense for the live line while `call` runs. */
export function stepActivity(call: ToolCall): string {
  return getPresenter(call.name).activity?.(call) ?? inferActivity(call);
}
