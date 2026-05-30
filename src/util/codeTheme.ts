// Keeps a single <link> to the selected highlight.js theme stylesheet in sync
// with the chosen family AND the app's light/dark mode.
//
// We import only the ~12 curated theme CSS files as URLs (tiny; not the full
// 80-theme set), so the bundle stays lean. The theme stylesheets put their
// background/padding on `.hljs` / `code.hljs` — which our editor's <pre>
// deliberately does NOT carry — so only the standalone `.hljs-*` token colors
// apply. The panel keeps its own background and exact layout.
import { useEffect } from "react";
import { useAppStore } from "../store";
import { CODE_THEMES } from "../data/codeThemes";

import githubDark from "highlight.js/styles/github-dark.css?url";
import github from "highlight.js/styles/github.css?url";
import atomOneDark from "highlight.js/styles/atom-one-dark.css?url";
import atomOneLight from "highlight.js/styles/atom-one-light.css?url";
import tokyoNightDark from "highlight.js/styles/tokyo-night-dark.css?url";
import tokyoNightLight from "highlight.js/styles/tokyo-night-light.css?url";
import solarizedDark from "highlight.js/styles/base16/solarized-dark.css?url";
import solarizedLight from "highlight.js/styles/base16/solarized-light.css?url";
import stackoverflowDark from "highlight.js/styles/stackoverflow-dark.css?url";
import stackoverflowLight from "highlight.js/styles/stackoverflow-light.css?url";
import a11yDark from "highlight.js/styles/a11y-dark.css?url";
import a11yLight from "highlight.js/styles/a11y-light.css?url";

const THEME_URLS: Record<string, string> = {
  "github-dark": githubDark,
  github,
  "atom-one-dark": atomOneDark,
  "atom-one-light": atomOneLight,
  "tokyo-night-dark": tokyoNightDark,
  "tokyo-night-light": tokyoNightLight,
  "base16/solarized-dark": solarizedDark,
  "base16/solarized-light": solarizedLight,
  "stackoverflow-dark": stackoverflowDark,
  "stackoverflow-light": stackoverflowLight,
  "a11y-dark": a11yDark,
  "a11y-light": a11yLight,
};

const LINK_ID = "hljs-code-theme";

/** Sync the highlight.js theme stylesheet with the chosen family + app theme.
 *  Returns true when the built-in Quorum palette should drive token colors
 *  (so the caller can tag the editor with the gating class). */
export function useHljsTheme(): boolean {
  const codeTheme = useAppStore((s) => s.codeTheme);
  const appTheme = useAppStore((s) => s.theme);

  useEffect(() => {
    const fam = CODE_THEMES.find((t) => t.id === codeTheme) ?? CODE_THEMES[0];
    const stem = appTheme === "light" ? fam.light : fam.dark;
    const href = stem ? THEME_URLS[stem] : undefined;
    const existing = document.getElementById(LINK_ID) as HTMLLinkElement | null;

    if (!href) {
      existing?.remove();
      return;
    }
    const link =
      existing ??
      (() => {
        const el = document.createElement("link");
        el.id = LINK_ID;
        el.rel = "stylesheet";
        document.head.appendChild(el);
        return el;
      })();
    if (link.getAttribute("href") !== href) link.setAttribute("href", href);
  }, [codeTheme, appTheme]);

  return codeTheme === "quorum";
}
