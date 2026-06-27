import { describe, expect, it } from "vitest";
import { aliasToolInput } from "./reducer-helpers";

describe("aliasToolInput", () => {
  const PI = [["path", "file_path"]] as const;
  const OPENCODE = [
    ["filePath", "file_path"],
    ["oldString", "old_string"],
    ["newString", "new_string"],
  ] as const;

  it("copies a string field to its alias", () => {
    expect(aliasToolInput({ path: "note.txt" }, PI)).toEqual({
      path: "note.txt",
      file_path: "note.txt",
    });
  });

  it("applies multiple aliases at once", () => {
    expect(aliasToolInput({ filePath: "/a", oldString: "x", newString: "y" }, OPENCODE)).toEqual({
      filePath: "/a",
      oldString: "x",
      newString: "y",
      file_path: "/a",
      old_string: "x",
      new_string: "y",
    });
  });

  it("returns the input untouched (same reference) when nothing aliases", () => {
    const input = { command: "ls -la" };
    expect(aliasToolInput(input, PI)).toBe(input);
  });

  it("does not overwrite a target field that's already set", () => {
    const input = { path: "from-path", file_path: "explicit" };
    expect(aliasToolInput(input, PI)).toBe(input);
  });

  it("ignores non-string source values", () => {
    const input = { path: 42 };
    expect(aliasToolInput(input, PI)).toBe(input);
  });

  it("only copies the aliases that apply", () => {
    expect(aliasToolInput({ filePath: "/a" }, OPENCODE)).toEqual({
      filePath: "/a",
      file_path: "/a",
    });
  });
});
