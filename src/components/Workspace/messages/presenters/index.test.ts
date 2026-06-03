import { describe, expect, it } from "vitest";

import { PRESENTERS, getPresenter } from "./index";
import { defaultPresenter } from "./default";

describe("getPresenter", () => {
  it("matches canonical Claude names", () => {
    expect(getPresenter("Bash")).toBe(PRESENTERS.Bash);
    expect(getPresenter("Read")).toBe(PRESENTERS.Read);
  });

  it("matches case-insensitively (cursor's lowercase names)", () => {
    expect(getPresenter("read")).toBe(PRESENTERS.Read);
    expect(getPresenter("glob")).toBe(PRESENTERS.Glob);
    expect(getPresenter("GREP")).toBe(PRESENTERS.Grep);
  });

  it("resolves cross-provider renames (cursor shell → Bash)", () => {
    expect(getPresenter("shell")).toBe(PRESENTERS.Bash);
  });

  it("falls back to the default presenter for unknown tools", () => {
    expect(getPresenter("someNovelTool")).toBe(defaultPresenter);
  });
});
