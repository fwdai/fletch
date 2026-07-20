import type { ITheme } from "@xterm/xterm";

/** Resolve a CSS custom property to a #rrggbb hex string.
 *  Works because the browser resolves oklch/hsl/etc to rgb() in getComputedStyle. */
export function resolveCSSVar(name: string): string {
  const el = document.createElement("span");
  el.style.cssText = `position:absolute;visibility:hidden;color:var(${name})`;
  document.body.appendChild(el);
  const rgb = getComputedStyle(el).color; // "rgb(r, g, b)"
  document.body.removeChild(el);
  const m = rgb.match(/rgb\((\d+),\s*(\d+),\s*(\d+)\)/);
  if (!m) return rgb;
  return `#${[m[1], m[2], m[3]].map((n) => parseInt(n, 10).toString(16).padStart(2, "0")).join("")}`;
}

/** Resolve --accent to an rgba() string with the given alpha (0–1).
 *  Used for selection highlight so it inherits the active palette tint. */
export function resolveAccentRgba(alpha: number): string {
  const hex = resolveCSSVar("--accent");
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

/** Build the full xterm theme from the current CSS variable values.
 *  Called at mount and whenever the dark/light class changes on <html>. */
export function resolveTheme(): ITheme {
  return {
    background: resolveCSSVar("--bg-1"),
    foreground: resolveCSSVar("--fg-1"),
    cursor: resolveCSSVar("--accent"),
    cursorAccent: resolveCSSVar("--bg-1"),
    selectionBackground: resolveAccentRgba(0.28),
    selectionInactiveBackground: resolveAccentRgba(0.14),
    black: resolveCSSVar("--fg-3"),
    red: resolveCSSVar("--danger"),
    green: resolveCSSVar("--success"),
    yellow: resolveCSSVar("--warn"),
    blue: resolveCSSVar("--info"),
    magenta: resolveCSSVar("--accent"),
    cyan: resolveCSSVar("--info"),
    white: resolveCSSVar("--fg-0"),
    brightBlack: resolveCSSVar("--fg-3"),
    brightRed: resolveCSSVar("--danger"),
    brightGreen: resolveCSSVar("--success"),
    brightYellow: resolveCSSVar("--warn"),
    brightBlue: resolveCSSVar("--info"),
    brightMagenta: resolveCSSVar("--accent"),
    brightCyan: resolveCSSVar("--info"),
    brightWhite: resolveCSSVar("--fg-0"),
  };
}
