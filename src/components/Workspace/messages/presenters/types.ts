import type { ReactNode } from "react";
import type { IconName } from "@/components/Icon";
import type { ChatItem } from "@/store";

export type ToolCall = Extract<ChatItem, { kind: "tool_call" }>;
export type ToolResult = Extract<ChatItem, { kind: "tool_result" }>;

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
}
