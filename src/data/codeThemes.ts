// Curated syntax-highlighting themes for the File panel. Each family ships a
// dark AND light highlight.js variant, so the code theme always follows the
// app's light/dark mode. "quorum" is the built-in palette (styled in app.css,
// also light/dark aware) and uses no external stylesheet.
export interface CodeThemeDef {
  id: string;
  label: string;
  /** highlight.js style stem for the app's dark / light mode; null for the
   *  built-in Quorum palette. */
  dark: string | null;
  light: string | null;
}

export const CODE_THEMES: CodeThemeDef[] = [
  { id: "quorum", label: "Fletch", dark: null, light: null },
  { id: "github", label: "GitHub", dark: "github-dark", light: "github" },
  { id: "atom-one", label: "Atom One", dark: "atom-one-dark", light: "atom-one-light" },
  { id: "tokyo-night", label: "Tokyo Night", dark: "tokyo-night-dark", light: "tokyo-night-light" },
  {
    id: "solarized",
    label: "Solarized",
    dark: "base16/solarized-dark",
    light: "base16/solarized-light",
  },
  {
    id: "stackoverflow",
    label: "Stack Overflow",
    dark: "stackoverflow-dark",
    light: "stackoverflow-light",
  },
  { id: "a11y", label: "Accessible", dark: "a11y-dark", light: "a11y-light" },
];
