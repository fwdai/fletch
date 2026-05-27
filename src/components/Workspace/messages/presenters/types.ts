import type { ReactNode } from "react";
import type { ChatItem } from "../../../../store";
import type { IconName } from "../../../Icon";

export type ToolCall = Extract<ChatItem, { kind: "tool_call" }>;
export type ToolResult = Extract<ChatItem, { kind: "tool_result" }>;

/** A tool-specific renderer. Two slots; the shared <ToolRow> chrome
 *  (icon, expand toggle, error tint) wraps them. */
export interface ToolPresenter {
  /** Optional icon override. Falls back to "wrench" (generic tool). */
  readonly icon?: IconName;
  /** Collapsed view. Should answer "what did this tool do?" at a glance. */
  summary(call: ToolCall, result: ToolResult | null): ReactNode;
  /** Body shown when expanded. Renders both call and result (when present). */
  expanded(call: ToolCall, result: ToolResult | null): ReactNode;
}
