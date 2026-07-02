import { describe, expect, it } from "vitest";
import { parseUnifiedDiff } from "@/util/diff";

describe("parseUnifiedDiff", () => {
  it("parses a modification hunk with correct old/new line numbers", () => {
    const diff = [
      "diff --git a/f.ts b/f.ts",
      "index 111..222 100644",
      "--- a/f.ts",
      "+++ b/f.ts",
      "@@ -1,3 +1,4 @@ func",
      " const a = 1;",
      "-const b = 2;",
      "+const b = 20;",
      "+const c = 3;",
      " return a;",
    ].join("\n");

    const hunks = parseUnifiedDiff(diff);
    expect(hunks).toHaveLength(1);
    expect(hunks[0].header).toBe("@@ -1,3 +1,4 @@ func");
    expect(hunks[0].lines).toEqual([
      { op: "ctx", o: 1, n: 1, t: "const a = 1;" },
      { op: "rem", o: 2, n: null, t: "const b = 2;" },
      { op: "add", o: null, n: 2, t: "const b = 20;" },
      { op: "add", o: null, n: 3, t: "const c = 3;" },
      { op: "ctx", o: 3, n: 4, t: "return a;" },
    ]);
  });

  it("ignores file headers and produces only hunk content", () => {
    const diff = [
      "diff --git a/new.ts b/new.ts",
      "new file mode 100644",
      "index 000..abc",
      "--- /dev/null",
      "+++ b/new.ts",
      "@@ -0,0 +1,2 @@",
      "+line one",
      "+line two",
    ].join("\n");

    const hunks = parseUnifiedDiff(diff);
    expect(hunks).toHaveLength(1);
    expect(hunks[0].lines).toEqual([
      { op: "add", o: null, n: 1, t: "line one" },
      { op: "add", o: null, n: 2, t: "line two" },
    ]);
  });

  it("ignores the \\ No newline at end of file marker", () => {
    const diff = [
      "@@ -1 +1 @@",
      "-old",
      "\\ No newline at end of file",
      "+new",
      "\\ No newline at end of file",
    ].join("\n");

    const hunks = parseUnifiedDiff(diff);
    expect(hunks[0].lines).toEqual([
      { op: "rem", o: 1, n: null, t: "old" },
      { op: "add", o: null, n: 1, t: "new" },
    ]);
  });

  it("splits multiple hunks", () => {
    const diff = ["@@ -1,1 +1,1 @@", "-a", "+A", "@@ -10,1 +10,1 @@ ctx", "-b", "+B"].join("\n");

    const hunks = parseUnifiedDiff(diff);
    expect(hunks).toHaveLength(2);
    expect(hunks[1].header).toBe("@@ -10,1 +10,1 @@ ctx");
    expect(hunks[1].lines).toEqual([
      { op: "rem", o: 10, n: null, t: "b" },
      { op: "add", o: null, n: 10, t: "B" },
    ]);
  });

  it("returns an empty array for empty input", () => {
    expect(parseUnifiedDiff("")).toEqual([]);
  });

  it("drops the trailing-newline artifact but keeps real blank context lines", () => {
    // git diff output ends with a newline (→ a phantom "" after split), while a
    // genuinely blank context line is encoded as a single space.
    const diff = "@@ -1,2 +1,2 @@\n a\n \n+b\n";
    const hunks = parseUnifiedDiff(diff);
    expect(hunks[0].lines).toEqual([
      { op: "ctx", o: 1, n: 1, t: "a" },
      { op: "ctx", o: 2, n: 2, t: "" },
      { op: "add", o: null, n: 3, t: "b" },
    ]);
  });
});
