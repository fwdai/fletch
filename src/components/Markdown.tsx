// Shared markdown renderer for chat content. Wraps react-markdown and
// overrides anchor rendering so links open in the user's default browser
// (via the Tauri shell plugin) instead of navigating inside the app window.

import type { AnchorHTMLAttributes } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { open as openExternal } from "@tauri-apps/plugin-shell";

function ExternalLink({
  href,
  children,
  ...rest
}: AnchorHTMLAttributes<HTMLAnchorElement>) {
  return (
    <a
      {...rest}
      href={href}
      onClick={(e) => {
        if (!href) return;
        e.preventDefault();
        void openExternal(href).catch(() => {});
      }}
    >
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
