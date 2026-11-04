import { describe, expect, it } from "vitest";
import { pickDefinition } from "./pickDefinition";

const defs = [{ id: "a" }, { id: "b" }, { id: "c" }];

describe("pickDefinition", () => {
  it("offers nothing while the project default is still being read", () => {
    expect(pickDefinition(defs, "", undefined)).toBeUndefined();
  });

  it("lands on the remembered default once it has been read", () => {
    expect(pickDefinition(defs, "", "b")).toEqual({ id: "b" });
  });

  it("falls back to the first definition when the project remembers none", () => {
    expect(pickDefinition(defs, "", null)).toEqual({ id: "a" });
  });

  it("falls back to the first definition when the remembered one is gone", () => {
    expect(pickDefinition(defs, "", "deleted")).toEqual({ id: "a" });
  });

  it("lets an explicit pick win, even over a read still in flight", () => {
    expect(pickDefinition(defs, "c", undefined)).toEqual({ id: "c" });
    expect(pickDefinition(defs, "c", "b")).toEqual({ id: "c" });
  });

  it("has nothing to offer without definitions", () => {
    expect(pickDefinition([], "", null)).toBeUndefined();
    expect(pickDefinition([], "a", null)).toBeUndefined();
  });
});
