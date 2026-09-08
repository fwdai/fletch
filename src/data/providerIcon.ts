// Fetching a provider's brand SVG off the website CDN, shared by the desktop
// chip (`components/ProviderIcon`) and the mobile mark (`ProviderMark`) so both
// surfaces resolve the same icon the same way — and a rebrand still only needs
// the SVG re-uploaded, no app release.
//
// Deliberately React-free: the two apps have their own React installs, so a
// hook living here would run against the wrong copy in one of them. Each side
// wraps these in its own tiny hook.

import { agentIconUrl } from "./providers";

/** Parsed-and-sanitized SVG markup, keyed by slug, so re-opening a pane
 *  doesn't re-fetch or flash. The webview's HTTP cache backs the network side;
 *  this just skips the empty frame on remount. */
const svgCache = new Map<string, string>();

/** Strip executable content from a trusted-origin SVG before inlining it:
 *  <script> elements, inline on* handlers, and <foreignObject> (which can host
 *  arbitrary HTML). Defense-in-depth — the markup comes from our own CDN. */
function sanitizeSvg(svg: string): string {
  return svg
    .replace(/<script[\s\S]*?<\/script>/gi, "")
    .replace(/<foreignObject[\s\S]*?<\/foreignObject>/gi, "")
    .replace(/\son\w+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/gi, "");
}

/** Already-fetched markup for `slug`, if any — lets a remount paint the icon on
 *  its first frame instead of flashing the monogram fallback. */
export function cachedProviderIcon(slug: string): string | null {
  return svgCache.get(slug) ?? null;
}

/** Fetch and sanitize a provider's brand icon, memoized by slug. Rejects when
 *  the icon is missing or unreachable (404, offline, network/CORS error), which
 *  callers render as the abbreviation monogram. */
export async function loadProviderIcon(slug: string, signal?: AbortSignal): Promise<string> {
  const cached = svgCache.get(slug);
  if (cached) return cached;
  const res = await fetch(agentIconUrl(slug), { signal });
  if (!res.ok) throw new Error(String(res.status));
  const clean = sanitizeSvg(await res.text());
  svgCache.set(slug, clean);
  return clean;
}
