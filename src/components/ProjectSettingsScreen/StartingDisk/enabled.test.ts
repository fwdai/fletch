import { describe, expect, it } from "vitest";
import type { ProjectFileReport } from "@/api";
import { FILE_LABEL } from "../projectFile/files";
import { enabledEdit, enabledSource } from "./enabled";

/** Both files, each saying `enabled` or not (`null`). */
const files = (shared: boolean | null, local: boolean | null): ProjectFileReport[] =>
  (
    [
      ["shared", shared],
      ["local", local],
    ] as const
  ).map(([which, enabled]) => ({
    path: FILE_LABEL[which],
    exists: true,
    error: null,
    warnings: [],
    allow: [],
    disable: [],
    seed: { enabled, exclude: [], warnings: [] },
  }));

describe("enabledSource", () => {
  it("is the local file once it says, else the team's", () => {
    expect(enabledSource(files(null, null))).toBeNull();
    expect(enabledSource(files(false, null))).toBe("shared");
    expect(enabledSource(files(false, true))).toBe("local");
    expect(enabledSource([])).toBeNull();
  });
});

describe("enabledEdit", () => {
  it("writes the chosen file while the local file says nothing", () => {
    expect(enabledEdit(files(null, null), "shared", false)).toEqual({
      file: "shared",
      change: { seedEnabled: false },
    });
    expect(enabledEdit(files(null, null), "local", false)).toEqual({
      file: "local",
      change: { seedEnabled: false },
    });
    expect(enabledEdit(files(false, null), "local", true)).toEqual({
      file: "local",
      change: { seedEnabled: true },
    });
  });

  it("takes the setting out when what lies beneath already says it", () => {
    expect(enabledEdit(files(false, null), "shared", true)).toEqual({
      file: "shared",
      change: { seedEnabled: null },
    });
    expect(enabledEdit(files(null, false), "local", true)).toEqual({
      file: "local",
      change: { seedEnabled: null },
    });
  });

  it("changes the local file once it says, whatever file is chosen", () => {
    expect(enabledEdit(files(false, true), "shared", false)).toEqual({
      file: "local",
      change: { seedEnabled: null },
    });
    expect(enabledEdit(files(false, false), "shared", true)).toEqual({
      file: "local",
      change: { seedEnabled: true },
    });
  });
});
