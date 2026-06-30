import type { ProviderId } from "../data/providers";
import { antigravityAdapter } from "./antigravity";
import { claudeAdapter } from "./claude";
import { codexAdapter } from "./codex";
import { cursorAdapter } from "./cursor";
import { opencodeAdapter } from "./opencode";
import { piAdapter } from "./pi";
import type { ChatAdapter } from "./types";

export { applyPolicy, modeFor } from "./policy";
export type { ChatAdapter, ChatItem, DisplayPolicy, NoticeSubtype, RawEvent } from "./types";

// Partial: not every provider is wired. Agents listed in PROVIDERS without an
// entry here are "coming soon" — the picker gates them via `hasAdapter` so they
// can never be selected and silently fall back to Claude.
export const ADAPTERS: Partial<Record<ProviderId, ChatAdapter>> = {
  antigravity: antigravityAdapter,
  claude: claudeAdapter,
  codex: codexAdapter,
  cursor: cursorAdapter,
  opencode: opencodeAdapter,
  pi: piAdapter,
};

export const DEFAULT_ADAPTER_ID: ProviderId = "claude";

/** True if `provider` has a real adapter (i.e. is runnable, not coming-soon). */
export function hasAdapter(provider: string | null | undefined): boolean {
  return !!provider && provider in ADAPTERS;
}

export function getAdapter(provider: string | null | undefined): ChatAdapter {
  if (provider) {
    const found = ADAPTERS[provider as ProviderId];
    if (found) return found;
    console.warn(
      `[adapters] unknown provider "${provider}", falling back to ${DEFAULT_ADAPTER_ID}`,
    );
  }
  const fallback = ADAPTERS[DEFAULT_ADAPTER_ID];
  if (!fallback) throw new Error(`missing adapter: ${DEFAULT_ADAPTER_ID}`);
  return fallback;
}
