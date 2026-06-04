import { claudeAdapter } from "./claude";
import { codexAdapter } from "./codex";
import { cursorAdapter } from "./cursor";
import { opencodeAdapter } from "./opencode";
import { piAdapter } from "./pi";
import type { ChatAdapter } from "./types";

export type { ChatAdapter, ChatItem, DisplayPolicy, NoticeSubtype, RawEvent } from "./types";
export { applyPolicy, modeFor } from "./policy";

export const ADAPTERS: Record<string, ChatAdapter> = {
  claude: claudeAdapter,
  codex: codexAdapter,
  cursor: cursorAdapter,
  opencode: opencodeAdapter,
  pi: piAdapter,
};

export const DEFAULT_ADAPTER_ID = "claude";

export function getAdapter(provider: string | null | undefined): ChatAdapter {
  if (provider) {
    const found = ADAPTERS[provider];
    if (found) return found;
    console.warn(
      `[adapters] unknown provider "${provider}", falling back to ${DEFAULT_ADAPTER_ID}`,
    );
  }
  return ADAPTERS[DEFAULT_ADAPTER_ID];
}
