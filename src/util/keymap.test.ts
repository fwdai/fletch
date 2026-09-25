import { describe, expect, it } from "vitest";
import {
  bindingProblem,
  comboFromEvent,
  effectiveCombos,
  formatCombo,
  GLOBAL_SHORTCUTS,
  matchesCombo,
  parseCombo,
  SHORTCUT_BY_ID,
  SHORTCUT_GROUPS,
  visibleShortcutGroups,
} from "./keymap";

function key(init: Partial<KeyboardEvent> & { key: string; code?: string }): KeyboardEvent {
  return {
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    code: "",
    ...init,
  } as KeyboardEvent;
}

describe("parseCombo", () => {
  it("splits modifiers from the key, keeping a punctuation key intact", () => {
    expect(parseCombo("Mod+Shift+,")).toEqual({ mod: true, shift: true, alt: false, key: "," });
    expect(parseCombo("Alt+ArrowUp")).toEqual({
      mod: false,
      shift: false,
      alt: true,
      key: "ArrowUp",
    });
    expect(parseCombo("j")).toEqual({ mod: false, shift: false, alt: false, key: "j" });
  });
});

describe("matchesCombo", () => {
  it("matches letters case-insensitively with the exact modifier set", () => {
    expect(matchesCombo(key({ key: "L", metaKey: true, shiftKey: true }), "Mod+Shift+L")).toBe(
      true,
    );
    expect(matchesCombo(key({ key: "l", ctrlKey: true, shiftKey: true }), "Mod+Shift+L")).toBe(
      true,
    );
    expect(matchesCombo(key({ key: "l", metaKey: true }), "Mod+Shift+L")).toBe(false);
    expect(matchesCombo(key({ key: "l", metaKey: true, altKey: true }), "Mod+L")).toBe(false);
  });

  it("matches punctuation on the physical key so Shift can't hide it", () => {
    const shifted = key({ key: "{", code: "BracketLeft", metaKey: true, shiftKey: true });
    expect(matchesCombo(shifted, "Mod+Shift+[")).toBe(true);
    expect(matchesCombo(shifted, "Mod+Shift+]")).toBe(false);
    expect(
      matchesCombo(key({ key: "?", code: "Slash", metaKey: true, shiftKey: true }), "Mod+Shift+/"),
    ).toBe(true);
  });

  it("matches named keys and Space by name", () => {
    expect(matchesCombo(key({ key: "Escape" }), "Escape")).toBe(true);
    expect(matchesCombo(key({ key: "Backspace", metaKey: true }), "Mod+Backspace")).toBe(true);
    expect(matchesCombo(key({ key: " " }), "Space")).toBe(true);
    expect(matchesCombo(key({ key: "ArrowUp", altKey: true }), "Alt+ArrowUp")).toBe(true);
  });
});

describe("formatCombo", () => {
  it("writes macOS chords with symbols and no separators", () => {
    expect(formatCombo("Mod+Shift+L", true)).toBe("⌘⇧L");
    expect(formatCombo("Alt+ArrowUp", true)).toBe("⌥↑");
    expect(formatCombo("Mod+Backspace", true)).toBe("⌘⌫");
    expect(formatCombo("Escape", true)).toBe("Esc");
    expect(formatCombo("Mod+,", true)).toBe("⌘,");
  });

  it("writes other platforms with named modifiers joined by +", () => {
    expect(formatCombo("Mod+Shift+L", false)).toBe("Ctrl+Shift+L");
    expect(formatCombo("Alt+ArrowDown", false)).toBe("Alt+↓");
    expect(formatCombo("Mod+Backspace", false)).toBe("Ctrl+Backspace");
    expect(formatCombo("Enter", false)).toBe("Enter");
  });
});

describe("comboFromEvent", () => {
  it("writes a chord the matcher accepts back", () => {
    const events = [
      key({ key: "k", code: "KeyK", metaKey: true }),
      key({ key: "L", code: "KeyL", ctrlKey: true, shiftKey: true }),
      key({ key: "{", code: "BracketLeft", metaKey: true, shiftKey: true }),
      key({ key: "?", code: "Slash", metaKey: true, shiftKey: true }),
      key({ key: "Backspace", code: "Backspace", metaKey: true }),
      key({ key: "ArrowUp", code: "ArrowUp", altKey: true }),
      key({ key: " ", code: "Space" }),
    ];
    for (const e of events) {
      const combo = comboFromEvent(e);
      expect(combo, e.key).not.toBeNull();
      expect(matchesCombo(e, combo as string), combo as string).toBe(true);
    }
    expect(comboFromEvent(key({ key: "k", code: "KeyK", metaKey: true }))).toBe("Mod+K");
    expect(
      comboFromEvent(key({ key: "{", code: "BracketLeft", metaKey: true, shiftKey: true })),
    ).toBe("Mod+Shift+[");
  });

  it("falls back to the physical key when a modifier turns the key into a symbol", () => {
    // ⌥N on macOS reports "˜"; ⌘⇧1 reports "!".
    expect(comboFromEvent(key({ key: "˜", code: "KeyN", altKey: true }))).toBe("Alt+N");
    expect(comboFromEvent(key({ key: "!", code: "Digit1", metaKey: true, shiftKey: true }))).toBe(
      "Mod+Shift+1",
    );
  });

  it("returns null for a lone modifier or an unnamed key", () => {
    expect(comboFromEvent(key({ key: "Meta", code: "MetaLeft", metaKey: true }))).toBeNull();
    expect(comboFromEvent(key({ key: "Shift", code: "ShiftLeft", shiftKey: true }))).toBeNull();
    expect(comboFromEvent(key({ key: "CapsLock", code: "CapsLock" }))).toBeNull();
  });
});

describe("overrides", () => {
  const search = SHORTCUT_BY_ID.search;

  it("uses the override when present and the defaults otherwise", () => {
    expect(effectiveCombos(search, {})).toEqual(["Mod+K"]);
    expect(effectiveCombos(search, { search: ["Mod+P"] })).toEqual(["Mod+P"]);
  });

  it("refuses a chord without a modifier", () => {
    expect(bindingProblem("search", "K", {})).toMatch(/need/);
    expect(bindingProblem("search", "Shift+K", {})).toMatch(/need/);
  });

  it("refuses chords the system menu owns", () => {
    expect(bindingProblem("search", "Mod+W", {})).toMatch(/system menu/);
    expect(bindingProblem("search", "Mod+Q", {})).toMatch(/system menu/);
  });

  it("refuses a chord another global shortcut answers to, defaults or override", () => {
    expect(bindingProblem("search", "Mod+B", {})).toMatch(/Toggle the sidebar/);
    expect(bindingProblem("search", "Mod+P", { home: ["Mod+P"] })).toMatch(/Home/);
    // Rebinding to its own current chord, or one moved away by an override, is fine.
    expect(bindingProblem("search", "Mod+K", {})).toBeNull();
    expect(bindingProblem("search", "Mod+B", { toggleSidebar: ["Mod+P"] })).toBeNull();
  });
});

describe("SHORTCUT_GROUPS", () => {
  it("gives every shortcut a unique id", () => {
    const ids = SHORTCUT_GROUPS.flatMap((g) => g.items.map((it) => it.id));
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("never binds one global chord to two actions", () => {
    const combos = GLOBAL_SHORTCUTS.flatMap((it) => it.combos);
    expect(new Set(combos).size).toBe(combos.length);
  });

  it("keeps global chords off the plain keys that text fields and lists own", () => {
    for (const it of GLOBAL_SHORTCUTS) {
      for (const combo of it.combos) {
        const c = parseCombo(combo);
        expect(c.mod || c.key === "Escape", `${it.id}: ${combo}`).toBe(true);
      }
    }
  });

  it("drops macOS-only bindings elsewhere", () => {
    const ids = (mac: boolean) =>
      visibleShortcutGroups(mac).flatMap((g) => g.items.map((it) => it.id));
    expect(ids(true)).toContain("dictation");
    expect(ids(false)).not.toContain("dictation");
  });
});
