import type { ProviderId } from "@/data/providers";
import { antigravityAdapter } from "./antigravity";
import { claudeAdapter } from "./claude";
import { codexAdapter } from "./codex";
import { cursorAdapter } from "./cursor";
import { opencodeAdapter } from "./opencode";
import { piAdapter } from "./pi";
import type { ChatAdapter } from "./types";

export { applyPolicy, modeFor } from "./policy";
export type { ChatAdapter, ChatItem, DisplayPolicy, NoticeSubtype, RawEvent } from "./types";

// Every provider is wired: the full Record makes adding a `ProviderId` without
// an adapter a compile error, keeping this registry in sync with the agent set.
export const ADAPTERS: Record<ProviderId, ChatAdapter> = {
  antigravity: antigravityAdapter,
  claude: claudeAdapter,
  codex: codexAdapter,
  cursor: cursorAdapter,
  opencode: opencodeAdapter,
  pi: piAdapter,
};

export const DEFAULT_ADAPTER_ID: ProviderId = "claude";

export function getAdapter(provider: string | null | undefined): ChatAdapter {
  if (provider) {
    const found = ADAPTERS[provider as ProviderId];
    if (found) return found;
    console.warn(
      `[adapters] unknown provider "${provider}", falling back to ${DEFAULT_ADAPTER_ID}`,
    );
  }
  return ADAPTERS[DEFAULT_ADAPTER_ID];
}
