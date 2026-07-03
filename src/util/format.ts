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

export function formatAge(iso: string, nowMs: number): string {
  const t = new Date(iso).getTime();
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
