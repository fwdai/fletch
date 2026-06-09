// Shared markdown renderer for chat content. Wraps react-markdown and
// overrides anchor rendering so links open in the user's default browser
// (via the Tauri shell plugin) instead of navigating inside the app window.

import type { AnchorHTMLAttributes, MouseEvent } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { open as openExternal } from "@tauri-apps/plugin-shell";

function ExternalLink({
  href,
  children,
  ...rest
}: AnchorHTMLAttributes<HTMLAnchorElement>) {
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

export function Markdown({ children }: { children: string }) {
  return (
    <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
      {children}
    </ReactMarkdown>
  );
}
