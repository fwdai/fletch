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

const DAY_MS = 86_400_000;

/** Local noon of the day containing `ms`. Stepping whole days from noon can't
 *  skip or repeat a date across a DST boundary, which is why every calendar
 *  helper below anchors here first. */
function localNoon(ms: number): number {
  const d = new Date(ms);
  d.setHours(12, 0, 0, 0);
  return d.getTime();
}

/** Every local day key from the day of `sinceMs` through the day of `untilMs`,
 *  inclusive, oldest first. */
export function dayKeysBetween(sinceMs: number, untilMs: number): string[] {
  const end = localNoon(untilMs);
  const out: string[] = [];
  for (let t = localNoon(sinceMs); t <= end; t += DAY_MS) out.push(localDay(t));
  return out;
}

/** The last `n` local day keys, oldest first, ending on the day of `nowMs`. */
export function recentDays(nowMs: number, n: number): string[] {
  return dayKeysBetween(localNoon(nowMs) - (n - 1) * DAY_MS, nowMs);
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

/** The local clock time of `ms` to the minute ("11:00", or "11:00 AM" where the
 *  locale is 12-hour). Used where a window boundary is an hour rather than a
 *  day and saying only the date would overstate the window. */
export function formatClockTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
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

/** A token count at one scale: raw under 1k, then k / M / B with one decimal.
 *  Billions matter for the all-sessions usage view, where a 90-day window
 *  across every local transcript runs to ten figures. */
export function formatTokens(n: number): string {
  if (n < 1_000) return `${n}`;
  if (n < 1_000_000) return `${(n / 1_000).toFixed(n < 10_000 ? 1 : 0)}k`;
  if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  return `${(n / 1_000_000_000).toFixed(1)}B`;
}

/** A dollar cost: 2 decimals at $1 and up ($5.00), sub-cent precision below $1
 *  ($0.034) so small sessions don't round to "$0.00". */
export function formatCost(usd: number): string {
  if (usd > 0 && usd < 0.01) return "<$0.01";
  return `$${usd.toFixed(usd < 1 ? 3 : 2)}`;
}

/** A 0–1 fraction as a percentage: whole percents at 1% and up ("42%"), one
 *  decimal below that ("0.4%"), and a floor marker under a tenth — the same
 *  shape as `formatCost`, so a small-but-real share never reads as nothing. */
export function formatPercent(fraction: number): string {
  const pct = fraction * 100;
  if (pct <= 0) return "0%";
  if (pct < 0.1) return "<0.1%";
  if (pct < 1) return `${pct.toFixed(1)}%`;
  return `${Math.round(pct)}%`;
}

/** A download size in decimal units (574 MB), the way the files themselves are
 *  advertised — a binary-unit "547 MiB" for the same bytes reads as a
 *  different download. One decimal below 10 of a unit, none above. */
export function formatBytes(bytes: number): string {
  const mb = bytes / 1_000_000;
  if (mb < 1) return `${Math.round(bytes / 1_000)} kB`;
  if (mb < 1_000) return `${mb.toFixed(mb < 10 ? 1 : 0)} MB`;
  const gb = mb / 1_000;
  return `${gb.toFixed(gb < 10 ? 1 : 0)} GB`;
}

/** Whole-percent progress, clamped to 0–100. `null` when the total is unknown
 *  or zero, which is the caller's cue for an indeterminate bar rather than a
 *  bogus 0%. */
export function downloadPercent(received: number, total: number | null): number | null {
  if (!total || total <= 0) return null;
  return Math.min(100, Math.max(0, Math.round((received / total) * 100)));
}
