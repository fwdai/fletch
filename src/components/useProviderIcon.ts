import { useEffect, useState } from "react";
import { cachedProviderIcon, loadProviderIcon } from "@/data/providerIcon";

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
  const [loaded, setLoaded] = useState<IconState>(() => initial(slug));

  // State carries the slug it belongs to and is reset *during* the render that
  // changes slug (React's "adjusting state when a prop changes"), not in the
  // effect that follows it. Resetting in the effect would commit one frame of
  // the previous provider's mark under the new provider's colour — visible
  // wherever one chip switches provider in place, e.g. the model picker.
  const current = loaded.slug === slug ? loaded : initial(slug);
  if (current !== loaded) setLoaded(current);

  // Always go through the loader, even when the render above found the icon
  // cached: effects flush after paint, so a sibling mark's shared request can
  // land in between, and a cache write is not a render. Skipping the loader on
  // a hit would strand this chip empty until it remounted. The loader answers a
  // hit from cache, and returning `prev` unchanged keeps that free of a render.
  useEffect(() => {
    let active = true;
    loadProviderIcon(slug)
      .then((svg) => {
        if (!active) return;
        setLoaded((prev) =>
          prev.slug === slug && prev.svg === svg ? prev : { slug, svg, failed: false },
        );
      })
      .catch(() => {
        if (!active) return;
        setLoaded((prev) =>
          prev.slug === slug && prev.failed ? prev : { slug, svg: null, failed: true },
        );
      });
    return () => {
      active = false;
    };
  }, [slug]);

  return { svg: current.svg, failed: current.failed };
}
