import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  loadBase,
  loadMode,
  parseBase,
  parseMode,
  presetReviewAllChanges,
  saveBase,
  saveMode,
} from "./prefs";

const storage = new Map<string, string>();

beforeEach(() => {
  storage.clear();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value),
  });
});

describe("parseMode", () => {
  it("keeps explore", () => {
    expect(parseMode("explore")).toBe("explore");
  });

  it("opens Changes for the retired modes, junk, and nothing stored", () => {
    for (const raw of ["changes", "files", "live", "", "Explore", null]) {
      expect(parseMode(raw)).toBe("changes");
    }
  });
});

describe("parseBase", () => {
  it("defaults to the fork point unless head is stored", () => {
    expect(parseBase("head")).toBe("head");
    expect(parseBase("fork")).toBe("fork");
    expect(parseBase(null)).toBe("fork");
    expect(parseBase("nonsense")).toBe("fork");
  });
});

describe("stored preferences", () => {
  it("round-trips the mode and the scope", () => {
    saveMode("explore");
    saveBase("head");
    expect(loadMode()).toBe("explore");
    expect(loadBase()).toBe("head");
  });

  it("reads a legacy stored mode as Changes", () => {
    storage.set("q2:codeMode", "live");
    expect(loadMode()).toBe("changes");
  });

  it("presets Changes in the All scope for the Git panel's cross-link", () => {
    saveMode("explore");
    saveBase("head");
    presetReviewAllChanges();
    expect(loadMode()).toBe("changes");
    expect(loadBase()).toBe("fork");
  });
});
