import { useProviderIcon } from "@/components/useProviderIcon";

interface ProviderIconProps {
  /** Provider/agent slug; builds the icon URL and identifies the agent. */
  slug: string;
  /** Abbreviation shown in the monogram fallback (e.g. "CC"). */
  short: string;
  /** Hue (oklch) tinting the chip border, background, and fallback text. */
  hue: number;
  size?: number;
}

/**
 * A provider's brand icon shown inside a hue-tinted chip. The SVG is fetched
 * from the website CDN (see `agentIconUrl` in `data/providers.ts`) and
 * inlined, so icons authored with `fill="currentColor"` inherit the chip's
 * theme-aware color (dark in light mode, light in dark mode) while full-color
 * logos keep their own fills. If the icon is missing or hasn't loaded —
 * offline first run, 404, network/CORS error — the chip falls back to the
 * abbreviation monogram. Because the URL is fixed, swapping the SVG on the CDN
 * updates the icon for everyone without an app release.
 */
export function ProviderIcon({ slug, short, hue, size = 30 }: ProviderIconProps) {
  const { svg, failed } = useProviderIcon(slug);

  const cls = ["chip-mono", "iflex-center", svg && !failed ? "has-brand-icon" : ""]
    .filter(Boolean)
    .join(" ");

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
