import { describe, expect, it } from "vitest";
import {
  formatCombo,
  GLOBAL_SHORTCUTS,
  matchesCombo,
  parseCombo,
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
