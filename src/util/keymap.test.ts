import { describe, expect, it, vi } from "vitest";
import {
  formatCombo,
  GLOBAL_SHORTCUTS,
  matchesCombo,
  parseCombo,
  SHORTCUT_GROUPS,
  visibleShortcutGroups,
} from "./keymap";

// The tests are written from a Mac: Mod is ⌘. One case below passes the
// platform explicitly to cover the other.
vi.mock("@/util/platform", () => ({ IS_MAC: true, IS_WINDOWS: false }));

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
    // Control is not Command on a Mac; on Windows and Linux it is the modifier
    // and the Windows key is not.
    expect(matchesCombo(key({ key: "l", ctrlKey: true, shiftKey: true }), "Mod+Shift+L")).toBe(
      false,
    );
    expect(
      matchesCombo(key({ key: "l", ctrlKey: true, shiftKey: true }), "Mod+Shift+L", false),
    ).toBe(true);
    expect(
      matchesCombo(key({ key: "l", metaKey: true, shiftKey: true }), "Mod+Shift+L", false),
    ).toBe(false);
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

  it("follows the layout's own keys, falling back to the physical key only for symbols", () => {
    // Dvorak: S on the physical semicolon, a comma on the physical W, and the
    // bracket on the physical minus — each is what the layout says it is.
    const dvorakS = key({ key: "s", code: "Semicolon", metaKey: true });
    expect(matchesCombo(dvorakS, "Mod+S")).toBe(true);
    expect(matchesCombo(dvorakS, "Mod+;")).toBe(false);
    const dvorakComma = key({ key: ",", code: "KeyW", metaKey: true });
    expect(matchesCombo(dvorakComma, "Mod+,")).toBe(true);
    expect(matchesCombo(dvorakComma, "Mod+W")).toBe(false);
    const dvorakBrace = key({ key: "{", code: "Minus", metaKey: true, shiftKey: true });
    expect(matchesCombo(dvorakBrace, "Mod+Shift+[")).toBe(true);
    expect(matchesCombo(dvorakBrace, "Mod+Shift+-")).toBe(false);
    // Option on macOS leaves no name (⌥N is "˜"); the physical key stands in.
    expect(
      matchesCombo(key({ key: "˜", code: "KeyN", altKey: true, metaKey: true }), "Mod+Alt+N"),
    ).toBe(true);
    // A layout with `!` on a key of its own, pressed without Shift, is not 1.
    const bareBang = key({ key: "!", code: "Slash", metaKey: true });
    expect(matchesCombo(bareBang, "Mod+1")).toBe(false);
    expect(matchesCombo(bareBang, "Mod+Shift+1")).toBe(false);
    // A keypress never answers to two chords, whatever the layout.
    const altLetter = key({ key: "x", code: "KeyY", altKey: true, metaKey: true });
    expect(matchesCombo(altLetter, "Mod+Alt+X")).toBe(true);
    expect(matchesCombo(altLetter, "Mod+Alt+Y")).toBe(false);
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
