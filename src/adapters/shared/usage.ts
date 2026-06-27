import type { TurnUsage } from "../types";

export interface UsageTokens {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  /** Dollar cost, only for agents that report it natively (opencode, pi). */
  costUsd?: number;
  model?: string;
}

/** Build a per-record TurnUsage from the four normalized token counts shared by
 *  claude/cursor/pi/opencode: applies the all-zero guard (returns undefined when
 *  the record carries no usage) and derives the default context-fill split
 *  `{ input, cacheRead, cacheWrite }`. Pass `costUsd`/`model` when the agent
 *  reports them. Codex differs (cumulative totals, derived fresh input and
 *  context) and builds its TurnUsage directly. */
export function buildTurnUsage({
  inputTokens,
  outputTokens,
  cacheReadTokens,
  cacheWriteTokens,
  costUsd,
  model,
}: UsageTokens): TurnUsage | undefined {
  if (inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens === 0) {
    return undefined;
  }
  return {
    inputTokens,
    outputTokens,
    cacheReadTokens,
    cacheWriteTokens,
    ...(costUsd !== undefined ? { costUsd } : {}),
    context: { input: inputTokens, cacheRead: cacheReadTokens, cacheWrite: cacheWriteTokens },
    ...(model !== undefined ? { model } : {}),
  };
}
