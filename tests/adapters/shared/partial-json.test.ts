import { describe, expect, it } from "vitest";
import { parsePartialJson } from "@/adapters/shared/partial-json";

// These pin the *configuration* we hand `partial-json` (which types may be
// partial) and the throw → `{}` fallback — not the library's own parsing,
// which it tests itself.
describe("parsePartialJson", () => {
  it("parses a complete document unchanged", () => {
    expect(parsePartialJson('{"file_path":"/a/b.ts","limit":8}')).toEqual({
      file_path: "/a/b.ts",
      limit: 8,
    });
  });

  it("fills in a string value as it arrives", () => {
    expect(parsePartialJson('{"file_path": "/Users/a')).toEqual({ file_path: "/Users/a" });
  });

  it("withholds a number until a delimiter proves it complete", () => {
    // `8` could still be growing into `80`.
    expect(parsePartialJson('{"file_path":"/a","limit":8')).toEqual({ file_path: "/a" });
    expect(parsePartialJson('{"file_path":"/a","limit":8,')).toEqual({
      file_path: "/a",
      limit: 8,
    });
  });

  it("drops a half-written key", () => {
    expect(parsePartialJson('{"file_path":"/a","old_st')).toEqual({ file_path: "/a" });
  });

  it("returns an empty bag rather than throwing on unusable input", () => {
    expect(parsePartialJson("")).toEqual({});
    expect(parsePartialJson("   ")).toEqual({});
    expect(parsePartialJson("{")).toEqual({});
    expect(parsePartialJson("not json at all")).toEqual({});
  });
});
