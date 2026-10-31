import type { ReactNode } from "react";
import type { IconName } from "@/components/Icon";
import type { ChatItem } from "@/store";

export type ToolCall = Extract<ChatItem, { kind: "tool_call" }>;
export type ToolResult = Extract<ChatItem, { kind: "tool_result" }>;

/** What a step is, as the one-line summary over a run of steps counts it:
 *  `change` writes files, `run` executes a command, `read` and `search` look
 *  around, `plan` updates the agent's checklist, and `other` is any tool
 *  without a dedicated presenter. */
export type StepKind = "change" | "run" | "read" | "search" | "plan" | "other";

/** What one tool call did, for the summary line of the run it sits in. */
export interface StepFacts {
  kind: StepKind;
  /** The file a change or read touched (full path; the summary shows its
   *  basename). */
  path?: string;
  additions?: number;
  deletions?: number;
  /** A short name for the step when the summary names it rather than counts
   *  it: the pattern a search looked for, the tool an `other` step used. */
  subject?: string;
}

/** A tool-specific renderer. Two slots; the shared <ToolRow> chrome
 *  (icon, expand toggle, error tint) wraps them. */
export interface ToolPresenter {
  /** Optional icon override. Falls back to "wrench" (generic tool). */
  readonly icon?: IconName;
  /** Optional display-name override. Falls back to the raw tool name. Used to
   *  give noisy names (e.g. MCP's `mcp__codegraph__…`) a human label. */
  readonly title?: string;
  /** Collapsed view. Should answer "what did this tool do?" at a glance. */
  summary(call: ToolCall, result: ToolResult | null): ReactNode;
  /** Body shown when expanded. Renders both call and result (when present). */
  expanded(call: ToolCall, result: ToolResult | null): ReactNode;
  /** What the call did, for the summary over its run of steps. Falls back to
   *  an `other` step named by the presenter's title (see `stepFacts`). */
  facts?(call: ToolCall, result: ToolResult | null): StepFacts;
  /** Present tense, for the live line while the call runs: "Reading
   *  index.tsx". Falls back to "Using <title>" (see `stepActivity`). */
  activity?(call: ToolCall): string;
}
