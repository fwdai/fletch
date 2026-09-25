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
  /** Global but not rebindable — Escape is a convention, not a preference. */
  fixed?: boolean;
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
        label: "Previous agent",
        description: "Select the row above the current one in the sidebar.",
      },
      {
        id: "nextAgent",
        combos: ["Mod+Shift+]"],
        label: "Next agent",
        description: "Select the row below the current one in the sidebar.",
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
        fixed: true,
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
      { id: "toggleSidebar", combos: ["Mod+["], label: "Toggle the sidebar" },
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

export const SHORTCUT_BY_ID: Readonly<Record<string, Shortcut>> = Object.fromEntries(
  SHORTCUT_GROUPS.flatMap((g) => g.items).map((it) => [it.id, it]),
);

// ---- User overrides ----------------------------------------------------------

/** The user's rebindings: shortcut id → the chords that replace its defaults.
 *  Only global shortcuts are rebindable; contextual ones are handled by their
 *  components and read this map not at all. */
export type ShortcutOverrides = Record<string, Combo[]>;

/** The chords `shortcut` answers to right now: the override when there is one,
 *  else its defaults. */
export function effectiveCombos(shortcut: Shortcut, overrides: ShortcutOverrides): Combo[] {
  return overrides[shortcut.id] ?? shortcut.combos;
}

/** Chords the native macOS menu owns; the app never sees them, so binding one
 *  would look like a shortcut that does nothing (or closes the window). */
const RESERVED_COMBOS: ReadonlySet<Combo> = new Set([
  "Mod+W",
  "Mod+Q",
  "Mod+M",
  "Mod+H",
  "Mod+Alt+H",
  "Mod+Z",
  "Mod+Shift+Z",
  "Mod+X",
  "Mod+C",
  "Mod+V",
  "Mod+A",
]);

/** Ids the user may rebind: global, and not one of the fixed conventions. */
export const REBINDABLE_IDS: ReadonlySet<string> = new Set(
  GLOBAL_SHORTCUTS.filter((it) => !it.fixed).map((it) => it.id),
);

/** Why `combo` can't be bound to shortcut `id`, or null when it can. A global
 *  chord needs a modifier so it never fights a text field for a plain key, and
 *  it can't be one a contextual surface already answers to — those listeners
 *  are fixed, so the chord would fire both. */
export function bindingProblem(
  id: string,
  combo: Combo,
  overrides: ShortcutOverrides,
): string | null {
  if (!parseCombo(combo).mod) return "Global shortcuts need ⌘ or Ctrl";
  if (RESERVED_COMBOS.has(combo)) return "Taken by the system menu";
  for (const group of SHORTCUT_GROUPS) {
    for (const other of group.items) {
      if (other.id === id) continue;
      const combos = group.global ? effectiveCombos(other, overrides) : other.combos;
      if (combos.includes(combo)) {
        return group.global
          ? `Already bound to “${other.label}”`
          : `Used by “${other.label}” (${group.label})`;
      }
    }
  }
  return null;
}

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

/** Punctuation is matched on the physical key: with Shift (or Option on macOS)
 *  held, `KeyboardEvent.key` reports the shifted character — `{` for ⌘⇧[ —
 *  and the chord would never match on `key`. */
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

/** The physical key for a letter or digit — `KeyN`, `Digit1`. */
function codeForChar(key: string): string {
  return /[0-9]/.test(key) ? `Digit${key}` : `Key${key.toUpperCase()}`;
}

/** Whether `e` is `combo`. `Mod` is either ⌘ or Ctrl on every platform — Ctrl
 *  chords on a Mac are harmless and this keeps one code path.
 *
 *  Letters and digits compare on `key`, what the layout produces, so a Dvorak
 *  ⌘S is the S the user typed. Except when a modifier has turned the key into
 *  a symbol — Option on macOS (⌥N is `˜`), Shift on a digit (⌘⇧1 is `!`) —
 *  where the physical key stands in, exactly as `comboFromEvent` records it.
 *  The fallback is limited to those cases so a plain ⌘O on Dvorak (physical
 *  KeyS) can't also answer to ⌘S. */
export function matchesCombo(e: KeyboardEvent, combo: Combo): boolean {
  const c = parseCombo(combo);
  if (c.mod !== (e.metaKey || e.ctrlKey)) return false;
  if (c.shift !== e.shiftKey) return false;
  if (c.alt !== e.altKey) return false;
  const code = PUNCTUATION_CODES[c.key];
  if (code) return e.code === code;
  if (c.key === "Space") return e.key === " ";
  if (c.key.length !== 1) return e.key === c.key;
  if (e.key.toLowerCase() === c.key.toLowerCase()) return true;
  const symbolic = e.altKey || (e.shiftKey && /[0-9]/.test(c.key));
  return symbolic && e.code === codeForChar(c.key);
}

/** Whether `combo` is well-formed: known modifiers, no repeats, and a key the
 *  map can match and format. Guards what comes back from storage; the recorder
 *  only ever produces well-formed chords. */
export function isValidCombo(combo: Combo): boolean {
  const parts = combo.split("+");
  const key = parts.pop() ?? "";
  if (parts.length !== new Set(parts).size) return false;
  if (!parts.every((p) => p === "Mod" || p === "Shift" || p === "Alt")) return false;
  return (
    /^[A-Za-z0-9]$/.test(key) ||
    key in PUNCTUATION_CODES ||
    key === "Space" ||
    NAMED_KEYS.has(key) ||
    /^F([1-9]|1[0-2])$/.test(key)
  );
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

/** The chord `e` is, written the way the map writes them — the recorder's
 *  half of `matchesCombo`. Null for a lone modifier or a key the map has no
 *  name for, so the recorder keeps waiting. Letters and digits come from
 *  `key` (what the layout produces, which is what the matcher compares) unless
 *  a modifier has turned it into a symbol; then the physical key stands in. */
export function comboFromEvent(e: KeyboardEvent): Combo | null {
  let key: string | null = null;
  if (/^[a-z0-9]$/i.test(e.key)) key = e.key.toUpperCase();
  else if (CODE_TO_PUNCTUATION[e.code]) key = CODE_TO_PUNCTUATION[e.code];
  else if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3);
  else if (/^Digit[0-9]$/.test(e.code)) key = e.code.slice(5);
  else if (e.key === " ") key = "Space";
  else if (NAMED_KEYS.has(e.key) || /^F([1-9]|1[0-2])$/.test(e.key)) key = e.key;
  if (!key) return null;
  const mods = [(e.metaKey || e.ctrlKey) && "Mod", e.altKey && "Alt", e.shiftKey && "Shift"];
  return [...mods.filter(Boolean), key].join("+");
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
