import { cachedProviderIcon, loadProviderIcon } from "@desktop/data/providerIcon";
import { useEffect, useState } from "react";

interface IconState {
  slug: string;
  svg: string | null;
  failed: boolean;
}

const initial = (slug: string): IconState => ({
  slug,
  svg: cachedProviderIcon(slug),
  failed: false,
});

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
  const [loaded, setLoaded] = useState<IconState>(() => initial(slug));

  // State carries the slug it belongs to and is reset *during* the render that
  // changes slug (React's "adjusting state when a prop changes"), not in the
  // effect that follows it. Resetting in the effect would commit one frame of
  // the previous provider's mark under the new provider's colour — the runner
  // sheet switches one mark in place like that.
  const current = loaded.slug === slug ? loaded : initial(slug);
  if (current !== loaded) setLoaded(current);

  useEffect(() => {
    if (cachedProviderIcon(slug)) return;
    let active = true;
    loadProviderIcon(slug)
      .then((svg) => {
        if (active) setLoaded({ slug, svg, failed: false });
      })
      .catch(() => {
        if (active) setLoaded({ slug, svg: null, failed: true });
      });
    return () => {
      active = false;
    };
  }, [slug]);

  return { svg: current.svg, failed: current.failed };
}
