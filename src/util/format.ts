// Display helpers shared across components.

export function basename(p: string): string {
  const parts = p.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

/** The directory portion of a path ("" for a top-level entry). */
export function parentDir(p: string): string {
  const i = p.lastIndexOf("/");
  return i === -1 ? "" : p.slice(0, i);
}

/** Join a directory and a name, tolerating the empty (root) directory. */
export function joinPath(dir: string, name: string): string {
  return dir ? `${dir}/${name}` : name;
}

export function firstLine(s: string, max = 56): string {
  const idx = s.indexOf("\n");
  const head = idx === -1 ? s : s.slice(0, idx);
  return head.length > max ? `${head.slice(0, max - 1)}…` : head;
}

/** How long ago `at` was, at one scale ("now", "42m", "6h", "3d"). Accepts an
 *  ISO string or an ms-epoch number — the app stores timestamps both ways. */
export function formatAge(at: string | number, nowMs: number): string {
  const t = typeof at === "number" ? at : new Date(at).getTime();
  if (Number.isNaN(t)) return "";
  const seconds = Math.max(0, Math.floor((nowMs - t) / 1000));
  if (seconds < 60) return "now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  return `${days}d`;
}

/** Up-to-two-letter avatar initials: first + last initial, falling back to
 *  the email's first character, then a neutral placeholder. */
export function accountInitials(first: string, last: string, email = ""): string {
  const combined = `${first.trim()[0] ?? ""}${last.trim()[0] ?? ""}`.toUpperCase();
  if (combined) return combined;
  const e = email.trim()[0];
  return e ? e.toUpperCase() : "?";
}

/** Local calendar date key (YYYY-MM-DD) — matches SQLite's
 *  `date(…, 'localtime')`, so frontend-derived days join cleanly against
 *  SQL-bucketed ones. */
export function localDay(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** The local Monday of the week containing `ms`, as a `localDay` key. Anchored
 *  at noon before stepping whole days, so a DST boundary can't land the result
 *  on the wrong date (the same trick the heatmap grid uses). */
export function weekStartDay(ms: number): string {
  const d = new Date(ms);
  d.setHours(12, 0, 0, 0);
  d.setDate(d.getDate() - ((d.getDay() + 6) % 7));
  return localDay(d.getTime());
}

/** A span of elapsed time at one significant scale: minutes under an hour,
 *  hours under a day, then days. One decimal below 10 of a unit ("4.2h") so
 *  short spans stay distinguishable, none above it ("14h"). */
export function formatDuration(ms: number): string {
  const minutes = Math.max(0, ms) / 60_000;
  if (minutes < 60) return `${Math.round(minutes)}m`;
  const hours = minutes / 60;
  if (hours < 24) return `${hours.toFixed(hours < 10 ? 1 : 0)}h`;
  const days = hours / 24;
  return `${days.toFixed(days < 10 ? 1 : 0)}d`;
}

export function formatTokens(n: number): string {
  if (n < 1_000) return `${n}`;
  if (n < 1_000_000) return `${(n / 1_000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

/** A dollar cost: 2 decimals at $1 and up ($5.00), sub-cent precision below $1
 *  ($0.034) so small sessions don't round to "$0.00". */
export function formatCost(usd: number): string {
  if (usd > 0 && usd < 0.01) return "<$0.01";
  return `$${usd.toFixed(usd < 1 ? 3 : 2)}`;
}
