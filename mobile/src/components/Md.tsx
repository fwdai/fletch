// Markdown for chat prose. The same renderer the desktop uses (react-markdown +
// remark-gfm), so an agent's headings, emphasis, lists, tables and fenced code
// read on the phone the way they read on the Mac — the hand-rolled subset this
// replaced knew only bold, inline code and "- " bullets, and printed the rest
// of the syntax raw.
//
// Links open outside the app: a webview that navigates away from the bundle has
// no way back, so anchors get the target/rel pair the rest of the app uses.
import type { AnchorHTMLAttributes } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

const components: Components = {
  a: ({ children, ...rest }: AnchorHTMLAttributes<HTMLAnchorElement>) => (
    <a {...rest} target="_blank" rel="noreferrer">
      {children}
    </a>
  ),
};

const plugins = [remarkGfm];

export function Md({ text }: { text: string }) {
  return (
    <ReactMarkdown remarkPlugins={plugins} components={components}>
      {text}
    </ReactMarkdown>
  );
}
