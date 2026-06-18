import type { ToolPresenter } from "./types";
import { agentPresenter } from "./Agent";
import { bashPresenter } from "./Bash";
import { readPresenter } from "./Read";
import { editPresenter } from "./Edit";
import { multiEditPresenter } from "./MultiEdit";
import { writePresenter } from "./Write";
import { grepPresenter } from "./Grep";
import { globPresenter } from "./Glob";
import { taskCreatePresenter } from "./TaskCreate";
import { defaultPresenter } from "./default";

export type { ToolPresenter, ToolCall, ToolResult } from "./types";

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

export function getPresenter(toolName: string): ToolPresenter {
  return BY_KEY[toolName.toLowerCase()] ?? defaultPresenter;
}
