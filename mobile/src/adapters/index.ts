// The desktop's chat adapters, re-registered for mobile.
//
// Why not import `@desktop/adapters` directly: each provider's `index.ts` also
// wires its `usage.ts`, whose type-only import of `@/adapters/usage` reaches
// `@/api` and therefore `@tauri-apps/api/event` — a module resolved from the
// desktop tree's own node_modules, which a standalone mobile install doesn't
// have. Mobile shows no token-usage surface, so it imports the three members it
// actually renders with (`normalizeTranscript`, `reduce`, `policy`) straight
// from each provider. The reducers themselves are shared verbatim; nothing is
// reimplemented here, and no desktop file is patched.

import { normalizeTranscript as agyNormalize } from "@desktop/adapters/antigravity/normalize";
import { antigravityPolicy } from "@desktop/adapters/antigravity/policy";
import { reduce as agyReduce } from "@desktop/adapters/antigravity/reduce";
import { normalizeTranscript as claudeNormalize } from "@desktop/adapters/claude/normalize";
import { claudePolicy } from "@desktop/adapters/claude/policy";
import { reduce as claudeReduce } from "@desktop/adapters/claude/reduce";
import { normalizeTranscript as codexNormalize } from "@desktop/adapters/codex/normalize";
import { codexPolicy } from "@desktop/adapters/codex/policy";
import { reduce as codexReduce } from "@desktop/adapters/codex/reduce";
import { normalizeTranscript as cursorNormalize } from "@desktop/adapters/cursor/normalize";
import { cursorPolicy } from "@desktop/adapters/cursor/policy";
import { reduce as cursorReduce } from "@desktop/adapters/cursor/reduce";
import { normalizeTranscript as opencodeNormalize } from "@desktop/adapters/opencode/normalize";
import { opencodePolicy } from "@desktop/adapters/opencode/policy";
import { reduce as opencodeReduce } from "@desktop/adapters/opencode/reduce";
import { normalizeTranscript as piNormalize } from "@desktop/adapters/pi/normalize";
import { piPolicy } from "@desktop/adapters/pi/policy";
import { reduce as piReduce } from "@desktop/adapters/pi/reduce";
import { applyPolicy } from "@desktop/adapters/policy";
import type { ChatItem, DisplayPolicy, RawEvent } from "@desktop/adapters/types";
import type { ProviderId } from "@desktop/data/providers";

export type { ChatItem, DisplayPolicy, RawEvent };
export { applyPolicy };

/** The subset of the desktop `ChatAdapter` mobile renders with. */
export interface MobileAdapter {
  readonly id: string;
  reduce(prev: ChatItem[], rawEvent: RawEvent): ChatItem[];
  normalizeTranscript(lines: unknown[]): RawEvent[];
  readonly policy: DisplayPolicy;
}

// A full Record keyed by ProviderId, so adding a provider on the desktop is a
// compile error here rather than a silently unrendered agent.
export const ADAPTERS: Record<ProviderId, MobileAdapter> = {
  claude: {
    id: "claude",
    reduce: claudeReduce,
    normalizeTranscript: claudeNormalize,
    policy: claudePolicy,
  },
  codex: {
    id: "codex",
    reduce: codexReduce,
    normalizeTranscript: codexNormalize,
    policy: codexPolicy,
  },
  cursor: {
    id: "cursor",
    reduce: cursorReduce,
    normalizeTranscript: cursorNormalize,
    policy: cursorPolicy,
  },
  opencode: {
    id: "opencode",
    reduce: opencodeReduce,
    normalizeTranscript: opencodeNormalize,
    policy: opencodePolicy,
  },
  pi: { id: "pi", reduce: piReduce, normalizeTranscript: piNormalize, policy: piPolicy },
  antigravity: {
    id: "antigravity",
    reduce: agyReduce,
    normalizeTranscript: agyNormalize,
    policy: antigravityPolicy,
  },
};

export const DEFAULT_ADAPTER_ID: ProviderId = "claude";

export function getAdapter(provider: string | null | undefined): MobileAdapter {
  return ADAPTERS[provider as ProviderId] ?? ADAPTERS[DEFAULT_ADAPTER_ID];
}
