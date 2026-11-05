import { describe, expect, it } from "vitest";
import { loadRecents, MAX_RECENTS, noteRecent } from "./recents";

function memory(initial: Record<string, string> = {}) {
  const map = new Map(Object.entries(initial));
  return {
    getItem: (k: string) => map.get(k) ?? null,
    setItem: (k: string, v: string) => void map.set(k, v),
  };
}

describe("recents", () => {
  it("starts empty and remembers what ran, newest first", () => {
    const store = memory();
    expect(loadRecents(store)).toEqual([]);
    noteRecent("newAgent", store);
    noteRecent("history", store);
    expect(loadRecents(store)).toEqual(["history", "newAgent"]);
  });

  it("moves a repeat to the front rather than listing it twice", () => {
    const store = memory();
    noteRecent("a", store);
    noteRecent("b", store);
    noteRecent("a", store);
    expect(loadRecents(store)).toEqual(["a", "b"]);
  });

  it("keeps the list bounded", () => {
    const store = memory();
    for (let i = 0; i < MAX_RECENTS + 3; i++) noteRecent(`c${i}`, store);
    const list = loadRecents(store);
    expect(list).toHaveLength(MAX_RECENTS);
    expect(list[0]).toBe(`c${MAX_RECENTS + 2}`);
  });

  it("shrugs off a damaged or foreign blob", () => {
    expect(loadRecents(memory({ "fletch:palette:recents": "{not json" }))).toEqual([]);
    expect(loadRecents(memory({ "fletch:palette:recents": '["ok", 3, null]' }))).toEqual(["ok"]);
    expect(loadRecents(memory({ "fletch:palette:recents": '{"a":1}' }))).toEqual([]);
  });
});
