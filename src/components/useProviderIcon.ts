import { useEffect, useState } from "react";
import { cachedProviderIcon, loadProviderIcon } from "@/data/providerIcon";

/**
 * A provider's brand SVG for `slug`, sanitized and cached (see
 * `data/providerIcon.ts`). Three states worth distinguishing: markup to inline,
 * `failed` — the icon is missing or unreachable, so render the monogram
 * fallback — and neither, meaning the fetch is still in flight and the chip
 * should stay empty rather than flash a monogram it is about to replace.
 *
 * The mobile app has the same hook over the same loader (`lib/useProviderIcon`);
 * only the React instance differs.
 */
export function useProviderIcon(slug: string): { svg: string | null; failed: boolean } {
  const [svg, setSvg] = useState<string | null>(() => cachedProviderIcon(slug));
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const cached = cachedProviderIcon(slug);
    setSvg(cached);
    setFailed(false);
    if (cached) return;
    const ctrl = new AbortController();
    let active = true;
    loadProviderIcon(slug, ctrl.signal)
      .then((markup) => {
        if (active) setSvg(markup);
      })
      .catch((e: Error) => {
        if (active && e.name !== "AbortError") setFailed(true);
      });
    return () => {
      active = false;
      ctrl.abort();
    };
  }, [slug]);

  return { svg, failed };
}
