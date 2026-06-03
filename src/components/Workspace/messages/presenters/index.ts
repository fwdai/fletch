import type { ToolPresenter } from "./types";
import { agentPresenter } from "./Agent";
import { bashPresenter } from "./Bash";
import { readPresenter } from "./Read";
import { editPresenter } from "./Edit";
import { multiEditPresenter } from "./MultiEdit";
import { writePresenter } from "./Write";
import { grepPresenter } from "./Grep";
import { globPresenter } from "./Glob";
import { defaultPresenter } from "./default";

export type { ToolPresenter, ToolCall, ToolResult } from "./types";

// Registry keyed by the tool name reported on `tool_call.name`. Adapters
// that emit different names for the same conceptual tool can either
// register an alias here or normalize the name in their reducer.
export const PRESENTERS: Record<string, ToolPresenter> = {
  Agent: agentPresenter,
  Bash: bashPresenter,
  // Codex names its shell tool `shell`; same UI as Claude's `Bash`.
  shell: bashPresenter,
  Read: readPresenter,
  Edit: editPresenter,
  MultiEdit: multiEditPresenter,
  Write: writePresenter,
  Grep: grepPresenter,
  Glob: globPresenter,
};

export function getPresenter(toolName: string): ToolPresenter {
  return PRESENTERS[toolName] ?? defaultPresenter;
}
