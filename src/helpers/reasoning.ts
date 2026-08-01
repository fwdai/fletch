// Codex persists reasoning encrypted in its rollout, so the readable text from
// the live item.completed event has to be stored separately for transcript
// replay. The rollout's matching reasoning item supplies its canonical order.

import type { RawEvent } from "../adapters";
import { api } from "../api";
import type { AppState } from "../store";
import { providerFor } from "./agentLookups";

export async function persistLiveReasoning(
  get: () => AppState,
  agentId: string,
  rawEvent: RawEvent,
): Promise<void> {
  if (providerFor(get(), agentId) !== "codex" || rawEvent.type !== "item.completed") return;
  const item =
    rawEvent.item && typeof rawEvent.item === "object" && !Array.isArray(rawEvent.item)
      ? (rawEvent.item as Record<string, unknown>)
      : undefined;
  if (
    item?.type !== "reasoning" ||
    typeof item.id !== "string" ||
    typeof item.text !== "string" ||
    !item.text
  ) {
    return;
  }

  try {
    await api.appendLiveRecord(agentId, "codex", `reasoning:${item.id}`, rawEvent);
  } catch {
    // Non-critical: carryForwardStoreOnly keeps the live text for this app
    // session even if durable persistence is temporarily unavailable.
  }
}
