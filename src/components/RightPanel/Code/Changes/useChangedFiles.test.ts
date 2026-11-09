import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FileStatus } from "@/api";

const { listChangedFiles, getState } = vi.hoisted(() => ({
  listChangedFiles: vi.fn(),
  getState: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { listChangedFiles } }));
vi.mock("@/store", () => ({ useAppStore: { getState } }));

import { fetchChangedFiles } from "./useChangedFiles";

const file = (path: string): FileStatus => ({
  path,
  kind: "modified",
  staged: false,
  additions: 1,
  deletions: 0,
});

const uncommitted = [file("dirty.ts")];

describe("fetchChangedFiles", () => {
  beforeEach(() => {
    listChangedFiles.mockReset();
    getState.mockReturnValue({ gitStates: { a1: { files: uncommitted } } });
  });

  it("returns the scope's list when the backend answers", async () => {
    const files = [file("committed.ts"), file("dirty.ts")];
    listChangedFiles.mockResolvedValue(files);

    await expect(fetchChangedFiles("a1", "fork")).resolves.toEqual({ files, degraded: false });
    expect(listChangedFiles).toHaveBeenCalledWith("a1", "fork");
  });

  it("falls back to the git poll's uncommitted list when the command is missing", async () => {
    listChangedFiles.mockRejectedValue(new Error("unknown command list_changed_files"));

    await expect(fetchChangedFiles("a1", "fork")).resolves.toEqual({
      files: uncommitted,
      degraded: true,
    });
  });

  it("degrades to an empty list when the git poll has nothing for the agent", async () => {
    listChangedFiles.mockRejectedValue("checkout not ready");

    await expect(fetchChangedFiles("other", "head")).resolves.toEqual({
      files: [],
      degraded: true,
    });
  });
});
