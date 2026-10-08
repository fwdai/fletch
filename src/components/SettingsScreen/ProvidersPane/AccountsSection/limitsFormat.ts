// The words around an account's limit meters: when a window resets, how old
// the reading is and where it came from, and what the last Refresh ran into.
// Pure over `nowMs` so the copy is testable without a clock.

import type { LimitSource, LimitsRefreshState, ProviderLimits } from "@/api/types/providers";
import { formatAge, formatClockTime } from "@/util/format";

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

/** Time left until a reset, at two units: "45m", "2h 13m", "3d 4h". Under a
 *  minute reads "<1m" rather than a countdown of seconds nobody can act on. */
export function formatCountdown(ms: number): string {
  if (ms < MINUTE_MS) return "<1m";
  const days = Math.floor(ms / DAY_MS);
  const hours = Math.floor((ms % DAY_MS) / HOUR_MS);
  const minutes = Math.floor((ms % HOUR_MS) / MINUTE_MS);
  if (days > 0) return hours > 0 ? `${days}d ${hours}h` : `${days}d`;
  if (hours > 0) return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
  return `${minutes}m`;
}

/** "resets 14:30 · in 2h 13m"; a reset more than a day out names its weekday
 *  ("resets Fri 09:00 · in 3d 4h"). A reset already passed means the reading
 *  predates it. Null when the source gave no reset time. */
export function resetLabel(resetsAt: number | null, nowMs: number): string | null {
  if (resetsAt === null) return null;
  const at = resetsAt * 1000;
  const left = at - nowMs;
  if (left <= 0) return "reset since this reading";
  const clock = formatClockTime(at);
  const when =
    left < DAY_MS
      ? clock
      : `${new Date(at).toLocaleDateString(undefined, { weekday: "short" })} ${clock}`;
  return `resets ${when} · in ${formatCountdown(left)}`;
}

/** Where a reading came from, in the muted freshness line. */
export const SOURCE_LABEL: Record<LimitSource, string> = {
  stream: "agent session",
  statusline: "status line",
  app_server: "Codex app-server",
  oauth_usage: "Claude usage page",
  rollout: "session log",
};

function agoLabel(asOf: number, nowMs: number): string {
  const age = formatAge(asOf * 1000, nowMs);
  return age === "now" ? "just now" : `${age} ago`;
}

/** "as of 5m ago · Codex app-server". */
export function asOfLabel(limits: ProviderLimits, nowMs: number): string {
  return `as of ${agoLabel(limits.as_of, nowMs)} · ${SOURCE_LABEL[limits.source]}`;
}

/** "Fable as of 2h ago" for model windows older than the reading they ride on
 *  — kept from the last Refresh while agent sessions updated the rest. Null
 *  when every model window is as fresh as the reading. */
export function olderModelsLabel(limits: ProviderLimits, nowMs: number): string | null {
  const older = (limits.models ?? []).filter((m) => m.as_of < limits.as_of);
  if (older.length === 0) return null;
  const asOf = Math.min(...older.map((m) => m.as_of));
  return `${older.map((m) => m.model).join(", ")} as of ${agoLabel(asOf, nowMs)}`;
}

/** What the last Refresh ran into, when it didn't produce a reading worth
 *  showing alone. Null for a refresh that went fine, or a back-off that has
 *  run out. */
export function refreshHint(refresh: LimitsRefreshState | null, nowMs: number): string | null {
  switch (refresh?.status) {
    case "rate_limited": {
      const until = refresh.next_allowed_at;
      if (until === null || until * 1000 <= nowMs) return null;
      return `Rate limited; try again at ${formatClockTime(until * 1000)}.`;
    }
    case "stale":
      return "Claude refused this login even after refreshing it. Sign in again to see limits.";
    case "signed_out":
      return "Signed out. Sign in to see limits.";
    default:
      return null;
  }
}

/** Whether a Refresh right now would only hand back the stored row: inside a
 *  429 back-off. Mirrors `refresh_allowed` in the engine's `agent::limits`,
 *  minus its 60 s floor, which the engine enforces quietly anyway. */
export function inBackoff(refresh: LimitsRefreshState | null, nowMs: number): boolean {
  return refresh?.next_allowed_at != null && refresh.next_allowed_at * 1000 > nowMs;
}
