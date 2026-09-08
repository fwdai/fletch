import { cachedProviderIcon, loadProviderIcon } from "@desktop/data/providerIcon";
import { useEffect, useState } from "react";

/**
 * A provider's brand SVG for `slug`, sanitized and cached by the shared loader
 * in `@desktop/data/providerIcon` — the same icons and cache semantics the
 * desktop chip uses. Three states: markup to inline, `failed` (missing or
 * unreachable, so render the monogram fallback), and neither, meaning the fetch
 * is still in flight and the mark should stay empty rather than flash a
 * monogram it is about to replace.
 *
 * A copy of the desktop's `components/useProviderIcon` on purpose: the two apps
 * have separate React installs, so a hook shared through `@desktop/*` would run
 * against the wrong copy. Everything with logic in it lives in the loader.
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
