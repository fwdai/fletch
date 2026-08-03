// Shared markdown renderer for chat content. Wraps react-markdown and
// overrides anchor rendering so links open in the user's default browser
// (via the Tauri shell plugin) instead of navigating inside the app window.

import { open as openExternal } from "@tauri-apps/plugin-shell";
import {
  type AnchorHTMLAttributes,
  createContext,
  type MouseEvent,
  useContext,
  useMemo,
} from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { remarkTokenChips } from "./markdownTokens";

function ExternalLink({ href, children, ...rest }: AnchorHTMLAttributes<HTMLAnchorElement>) {
  // Handle both primary clicks (onClick) and auxiliary clicks such as
  // middle-click (onAuxClick); the latter is not covered by onClick and
  // would otherwise let the webview navigate away from the app.
  const openInBrowser = (e: MouseEvent<HTMLAnchorElement>) => {
    if (!href) return;
    e.preventDefault();
    void openExternal(href).catch(() => {});
  };
  return (
    <a {...rest} href={href} onClick={openInBrowser} onAuxClick={openInBrowser}>
      {children}
    </a>
  );
}

const components: Components = { a: ExternalLink };

/** Tokens the surrounding surface wants rendered as clickable chips instead of
 *  prose — provided by the roadmap's PM chat, where the project's item codes
 *  link to the board beside it. Absent (every other chat) and this renderer is
 *  exactly what it was: the plugin isn't even installed.
 *
 *  A context rather than a prop because the renderer sits three shared
 *  components below the surface that knows the tokens (pane → transcript list →
 *  message row), none of which have any business carrying them. Clicks are
 *  handled by the provider on its own container, keyed on `TOKEN_CHIP_ATTR`. */
export const TokenChipContext = createContext<ReadonlySet<string> | null>(null);

export function Markdown({ children }: { children: string }) {
  const tokens = useContext(TokenChipContext);
  // Keyed on the set's identity, so a provider that holds it stable doesn't
  // recompile the pattern (or re-render the tree) on every unrelated change.
  const plugins = useMemo(
    () => (tokens && tokens.size > 0 ? [remarkGfm, remarkTokenChips(tokens)] : [remarkGfm]),
    [tokens],
  );
  return (
    <ReactMarkdown remarkPlugins={plugins} components={components}>
      {children}
    </ReactMarkdown>
  );
}
