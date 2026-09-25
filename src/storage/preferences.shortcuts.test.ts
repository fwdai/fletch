import { describe, expect, it } from "vitest";
import { parseShortcutOverrides } from "./preferences";

describe("parseShortcutOverrides", () => {
  it("reads a saved map back", () => {
    const raw = JSON.stringify({ search: ["Mod+P"], home: ["Mod+Shift+0", "Mod+0"] });
    expect(parseShortcutOverrides(raw)).toEqual({
      search: ["Mod+P"],
      home: ["Mod+Shift+0", "Mod+0"],
    });
  });

  it("drops ids the map no longer knows and malformed entries", () => {
    const raw = JSON.stringify({
      search: ["Mod+P", 3, ""],
      retired: ["Mod+X"],
      home: "Mod+0",
      usage: [],
    });
    expect(parseShortcutOverrides(raw)).toEqual({ search: ["Mod+P"] });
  });

  it("drops chords that aren't well-formed or lack a modifier, and ids that are fixed", () => {
    const raw = JSON.stringify({
      search: ["Cmd+P", "P", "Mod+Mod+P", "Mod+Shift+P"],
      escape: ["Mod+E"],
      send: ["Mod+Enter"],
      home: ["Shift+H"],
    });
    expect(parseShortcutOverrides(raw)).toEqual({ search: ["Mod+Shift+P"] });
  });

  it("reads a missing or corrupt blob as all defaults", () => {
    expect(parseShortcutOverrides(undefined)).toEqual({});
    expect(parseShortcutOverrides("")).toEqual({});
    expect(parseShortcutOverrides("{not json")).toEqual({});
    expect(parseShortcutOverrides("[1,2]")).toEqual({});
    expect(parseShortcutOverrides("null")).toEqual({});
  });
});
