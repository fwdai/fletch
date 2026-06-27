// Synchronous syntax highlighting for the File panel's editor. We re-run
// this on every keystroke (highlight-behind-textarea technique), so it must
// be fast and synchronous — highlight.js fits; Shiki (async/WASM) would not.
//
// The token COLORS come from the design's palette (see app.css: `.fp-hl
// .hljs-*`), so output matches the mockup while covering any language.
import hljs from "highlight.js/lib/common";
import { hljsLang } from "../data/languages";

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** Return highlight.js HTML for `text`. The visible characters are identical
 *  to the input (only `&<>` are escaped), so it overlays the textarea
 *  exactly. Falls back to escaped plaintext if highlighting throws. */
export function highlightToHtml(text: string, lang: string): string {
  const language = hljsLang(lang);
  try {
    if (language && hljs.getLanguage(language)) {
      return hljs.highlight(text, { language, ignoreIllegals: true }).value;
    }
    return hljs.highlightAuto(text).value;
  } catch {
    return escapeHtml(text);
  }
}
