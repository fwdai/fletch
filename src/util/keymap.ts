// The app's keyboard map, as data. One place that both the global handler
// (`util/shortcuts.ts`) and Settings › Shortcuts read, so what the list says a
// key does and what the key does can't drift apart. Contextual bindings (the
// composer's Enter, the sidebar's arrows, Mission Control's j/k) are handled by
// their own components; they are listed here for the reader, not dispatched.

import { IS_MAC } from "@/util/platform";

/** A chord written as `Mod+Shift+K`: zero or more of `Mod` (⌘ on macOS, Ctrl
 *  elsewhere), `Shift` and `Alt`, then one key — a `KeyboardEvent.key` name
 *  (`ArrowUp`, `Escape`, `Enter`, `Backspace`, `Home`) or a single character. */
export type Combo = string;

export interface Shortcut {
  /** Stable id: the global handler's action table is keyed by it. */
  id: string;
  /** Alternatives, all doing the same thing (`↑` and `k`). */
  combos: Combo[];
  label: string;
  /** When the chord applies, or what it does beyond the label. */
  description?: string;
  /** Only offered on macOS (dictation depends on Apple's recognizer). */
  macOnly?: boolean;
}

export interface ShortcutGroup {
  label: string;
  /** Dispatched from the window-level handler, anywhere in the app. Groups
   *  without it are handled by the component they belong to. */
  global?: boolean;
  items: Shortcut[];
}

export const SHORTCUT_GROUPS: readonly ShortcutGroup[] = [
  {
    label: "Navigation",
    global: true,
    items: [
      {
        id: "search",
        combos: ["Mod+K"],
        label: "Search agents",
        description:
          "Focus the sidebar search, showing the sidebar if hidden. ↓ steps into the list.",
      },
      {
        id: "prevAgent",
        combos: ["Mod+Shift+["],
        label: "Previous in the sidebar",
        description:
          "Select the row above — agents, drafts and workflow runs, in the order shown. Shows the sidebar if hidden.",
      },
      {
        id: "nextAgent",
        combos: ["Mod+Shift+]"],
        label: "Next in the sidebar",
        description: "Select the row below, wrapping at the end.",
      },
      {
        id: "home",
        combos: ["Mod+Shift+H"],
        label: "Home",
        description: "Leave the current agent for Home and its review queue.",
      },
      {
        id: "history",
        combos: ["Mod+Y"],
        label: "History",
        description: "Open or close the archived agents list.",
      },
      {
        id: "usage",
        combos: ["Mod+Shift+U"],
        label: "Usage",
        description: "Open or close the token usage report.",
      },
      {
        id: "quickSettings",
        combos: ["Mod+,"],
        label: "Quick settings",
        description: "Theme, panels and providers in a popover; All settings from there.",
      },
      {
        id: "projectSettings",
        combos: ["Mod+Shift+,"],
        label: "Project settings",
        description: "Open the current project's page on its Settings tab.",
      },
      {
        id: "shortcuts",
        combos: ["Mod+/"],
        label: "Keyboard shortcuts",
        description: "Open this list.",
      },
      {
        id: "escape",
        combos: ["Escape"],
        label: "Close",
        description: "Dismiss the open screen, popover or menu.",
      },
    ],
  },
  {
    label: "Agents",
    global: true,
    items: [
      {
        id: "newAgent",
        combos: ["Mod+N"],
        label: "New agent",
        description: "Start a draft in the project an agent was last started in.",
      },
      {
        id: "addProject",
        combos: ["Mod+O"],
        label: "Add project",
        description: "Open a folder, clone from GitHub, or create a new repository.",
      },
      {
        id: "focusComposer",
        combos: ["Mod+L"],
        label: "Focus the composer",
        description: "Put the caret in the message box of the open agent or draft.",
      },
      {
        id: "openInEditor",
        combos: ["Mod+Shift+O"],
        label: "Open in editor",
        description: "Open the agent's checkout in the editor last picked from the title bar.",
      },
      {
        id: "stopAgent",
        combos: ["Mod+."],
        label: "Stop the agent",
        description: "Interrupt the selected agent's turn.",
      },
      {
        id: "archiveAgent",
        combos: ["Mod+Backspace"],
        label: "Archive the agent",
        description: "Archive the selected agent once it is idle. Restore it from History.",
      },
    ],
  },
  {
    label: "Panels",
    global: true,
    items: [
      // Left bracket, left rail; right bracket, right rail. Shift on the same
      // keys steps through agents, so the brackets are one small family.
      // ⌘B stays as an alternative: it was the binding before the brackets, and
      // it is what VS Code and Cursor hands reach for.
      { id: "toggleSidebar", combos: ["Mod+[", "Mod+B"], label: "Toggle the sidebar" },
      { id: "togglePanel", combos: ["Mod+]"], label: "Toggle the side panel" },
      {
        id: "panelCode",
        combos: ["Mod+1"],
        label: "Code panel",
        description: "Show the Code tab, opening the side panel if hidden.",
      },
      { id: "panelGit", combos: ["Mod+2"], label: "Git panel" },
      { id: "panelRun", combos: ["Mod+3"], label: "Run panel" },
      { id: "panelTerminal", combos: ["Mod+4"], label: "Terminal panel" },
      { id: "toggleTheme", combos: ["Mod+Shift+L"], label: "Switch light / dark theme" },
    ],
  },
  {
    label: "Chat",
    items: [
      { id: "send", combos: ["Enter"], label: "Send the message" },
      { id: "newline", combos: ["Shift+Enter"], label: "New line" },
      { id: "find", combos: ["Mod+F"], label: "Find in the conversation" },
      {
        id: "prevTurn",
        combos: ["Alt+ArrowUp"],
        label: "Previous message",
        description: "Scroll to the user turn above, from anywhere but a text field.",
      },
      { id: "nextTurn", combos: ["Alt+ArrowDown"], label: "Next message" },
      {
        id: "dictation",
        combos: ["Mod+Shift+D"],
        label: "Dictate",
        description: "Start or stop dictation into the composer.",
        macOnly: true,
      },
      {
        id: "cancel",
        combos: ["Escape"],
        label: "Cancel",
        description: "Discard a dictation in progress, or stop the turn while it is stopping.",
      },
    ],
  },
  {
    label: "Sidebar list (with a row focused)",
    items: [
      { id: "rowStep", combos: ["ArrowUp", "ArrowDown"], label: "Move between rows" },
      { id: "rowEnds", combos: ["Home", "End"], label: "First / last row" },
      { id: "rowOpen", combos: ["Enter", "Space"], label: "Open the row" },
    ],
  },
  {
    label: "Mission Control (Home)",
    items: [
      { id: "queueStep", combos: ["j", "k"], label: "Move between review cards" },
      { id: "queueReview", combos: ["Enter"], label: "Review the card" },
      { id: "queueApprove", combos: ["a"], label: "Approve" },
      { id: "queueChanges", combos: ["r"], label: "Request changes" },
    ],
  },
  {
    label: "Side panel",
    items: [
      {
        id: "save",
        combos: ["Mod+S"],
        label: "Save the file",
        description: "Code tab. Edits autosave; this flushes the pending one.",
      },
      {
        id: "commit",
        combos: ["Mod+Enter"],
        label: "Commit",
        description: "Git tab, from the commit message. Also submits review and feedback forms.",
      },
      { id: "findTerminal", combos: ["Mod+F"], label: "Search the terminal" },
    ],
  },
];

/** The shortcuts the window-level handler dispatches, by id. */
export const GLOBAL_SHORTCUTS: readonly Shortcut[] = SHORTCUT_GROUPS.filter(
  (g) => g.global,
).flatMap((g) => g.items);

/** The groups worth showing on this platform. */
export function visibleShortcutGroups(mac = IS_MAC): ShortcutGroup[] {
  return SHORTCUT_GROUPS.map((g) => ({
    ...g,
    items: g.items.filter((it) => mac || !it.macOnly),
  }));
}

interface ParsedCombo {
  mod: boolean;
  shift: boolean;
  alt: boolean;
  key: string;
}

export function parseCombo(combo: Combo): ParsedCombo {
  const parts = combo.split("+");
  const key = parts.pop() ?? "";
  const mods = new Set(parts);
  return { mod: mods.has("Mod"), shift: mods.has("Shift"), alt: mods.has("Alt"), key };
}

/** The punctuation the map may name, with the physical key each sits on in the
 *  US layout — the stand-in when a modifier has left `key` with no name. */
const PUNCTUATION_CODES: Record<string, string> = {
  "[": "BracketLeft",
  "]": "BracketRight",
  ",": "Comma",
  ".": "Period",
  "/": "Slash",
  "\\": "Backslash",
  "`": "Backquote",
  "-": "Minus",
  "=": "Equal",
  ";": "Semicolon",
  "'": "Quote",
};

/** The platform's command modifier: ⌘ on macOS, Ctrl elsewhere. The other one
 *  is not a stand-in — a ⌃ chord on a Mac is its own thing (Emacs bindings in
 *  a text field), and the Windows key is never a shortcut modifier. */
export function isModDown(e: KeyboardEvent, mac = IS_MAC): boolean {
  return mac ? e.metaKey : e.ctrlKey;
}

/** Whether `e` is `combo`. The key is whatever [`keyOf`] says the event is, so
 *  a keypress answers to exactly one chord. */
export function matchesCombo(e: KeyboardEvent, combo: Combo, mac = IS_MAC): boolean {
  const c = parseCombo(combo);
  if (c.mod !== isModDown(e, mac)) return false;
  if (c.shift !== e.shiftKey) return false;
  if (c.alt !== e.altKey) return false;
  const key = keyOf(e);
  if (key === null) return false;
  return key.length === 1 ? key === c.key.toUpperCase() : key === c.key;
}

const CODE_TO_PUNCTUATION: Record<string, string> = Object.fromEntries(
  Object.entries(PUNCTUATION_CODES).map(([key, code]) => [code, key]),
);

const NAMED_KEYS: ReadonlySet<string> = new Set([
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Enter",
  "Backspace",
  "Delete",
  "Escape",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "Tab",
]);

/** What Shift makes of each unshifted key on a US layout, inverted, so a
 *  shifted press still names the key the layout has there. */
const UNSHIFTED: Record<string, string> = {
  "!": "1",
  "@": "2",
  "#": "3",
  $: "4",
  "%": "5",
  "^": "6",
  "&": "7",
  "*": "8",
  "(": "9",
  ")": "0",
  "{": "[",
  "}": "]",
  "<": ",",
  ">": ".",
  "?": "/",
  "|": "\\",
  "~": "`",
  _: "-",
  "+": "=",
  ":": ";",
  '"': "'",
};

/** The one key name `e` stands for, as the map writes it, or null for a lone
 *  modifier or a key the map has no name for. The layout's own key first (a
 *  Dvorak ⌘S is S, its ⌘, is a comma wherever the key sits, ⌘⇧1 is 1), and
 *  only when a modifier has turned the key into something with no name
 *  (Option on macOS: ⌥N is `˜`) does the physical key stand in. One answer
 *  per keypress, so nothing can match two chords. */
export function keyOf(e: KeyboardEvent): string | null {
  // Unshifting undoes Shift, so it only applies while Shift is held: a layout
  // that has `!` on a key of its own must not read as the digit under US-`!`.
  const typed = (e.shiftKey && UNSHIFTED[e.key]) || e.key;
  if (/^[a-z0-9]$/i.test(typed)) return typed.toUpperCase();
  if (typed in PUNCTUATION_CODES) return typed;
  if (typed === " ") return "Space";
  if (NAMED_KEYS.has(typed) || /^F([1-9]|1[0-2])$/.test(typed)) return typed;
  const punctuation = CODE_TO_PUNCTUATION[e.code];
  if (punctuation) return punctuation;
  if (/^Key[A-Z]$/.test(e.code)) return e.code.slice(3);
  if (/^Digit[0-9]$/.test(e.code)) return e.code.slice(5);
  return null;
}

const MAC_KEYS: Record<string, string> = {
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Enter: "↵",
  Backspace: "⌫",
  Escape: "Esc",
};

const OTHER_KEYS: Record<string, string> = {
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Escape: "Esc",
};

/** A chord as the platform writes it: `⌘⇧L` on macOS, `Ctrl+Shift+L` elsewhere.
 *  Modifier order follows the app's existing hints (⌘ first), not Apple's menus. */
export function formatCombo(combo: Combo, mac = IS_MAC): string {
  const c = parseCombo(combo);
  const key = c.key.length === 1 ? c.key.toUpperCase() : c.key;
  if (mac) {
    const mods = `${c.mod ? "⌘" : ""}${c.alt ? "⌥" : ""}${c.shift ? "⇧" : ""}`;
    return mods + (MAC_KEYS[key] ?? key);
  }
  const parts = [c.mod && "Ctrl", c.alt && "Alt", c.shift && "Shift", OTHER_KEYS[key] ?? key];
  return parts.filter(Boolean).join("+");
}
