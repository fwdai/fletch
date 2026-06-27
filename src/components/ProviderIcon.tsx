import { useEffect, useState } from "react";
import { agentIconUrl } from "../data/providers";

interface ProviderIconProps {
  /** Provider/agent slug; builds the icon URL and identifies the agent. */
  slug: string;
  /** Abbreviation shown in the monogram fallback (e.g. "CC"). */
  short: string;
  /** Hue (oklch) tinting the chip border, background, and fallback text. */
  hue: number;
  size?: number;
}

/** Parsed-and-sanitized SVG markup, keyed by slug, so re-opening the pane
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

/**
 * A provider's brand icon shown inside a hue-tinted chip. The SVG is fetched
 * from the website CDN (https://quorum.fwdai.org/agents/<slug>.svg) and
 * inlined, so icons authored with `fill="currentColor"` inherit the chip's
 * theme-aware color (dark in light mode, light in dark mode) while full-color
 * logos keep their own fills. If the icon is missing or hasn't loaded —
 * offline first run, 404, network/CORS error — the chip falls back to the
 * abbreviation monogram. Because the URL is fixed, swapping the SVG on the CDN
 * updates the icon for everyone without an app release.
 */
export function ProviderIcon({ slug, short, hue, size = 30 }: ProviderIconProps) {
  const [svg, setSvg] = useState<string | null>(() => svgCache.get(slug) ?? null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const cached = svgCache.get(slug);
    if (cached) {
      setSvg(cached);
      setFailed(false);
      return;
    }
    setSvg(null);
    setFailed(false);
    const ctrl = new AbortController();
    let active = true;
    fetch(agentIconUrl(slug), { signal: ctrl.signal })
      .then((r) => {
        if (!r.ok) throw new Error(String(r.status));
        return r.text();
      })
      .then((text) => {
        const clean = sanitizeSvg(text);
        svgCache.set(slug, clean);
        if (active) setSvg(clean);
      })
      .catch((e) => {
        if (active && e.name !== "AbortError") setFailed(true);
      });
    return () => {
      active = false;
      ctrl.abort();
    };
  }, [slug]);

  const cls = ["chip-mono", svg && !failed ? "has-brand-icon" : ""].filter(Boolean).join(" ");

  return (
    <span
      className={cls}
      style={{
        width: size,
        height: size,
        // Scale the corner radius and monogram with `size` so the chip stays
        // proportionate at any scale. The ratios reproduce the CSS defaults
        // (7px radius, 10.5px text) exactly at the 30px settings size.
        borderRadius: Math.max(3, Math.round(size * 0.233)),
        fontSize: Math.round(size * 0.35 * 10) / 10,
        ["--ph-h" as string]: hue,
        ["--ph" as string]: "oklch(.65 .13 var(--ph-h))",
      }}
    >
      {failed ? (
        short
      ) : svg ? (
        <span className="chip-mono-svg" dangerouslySetInnerHTML={{ __html: svg }} />
      ) : null}
    </span>
  );
}
