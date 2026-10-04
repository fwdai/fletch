import type { DownloadEvent } from "@tauri-apps/plugin-updater";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { checkForUpdate, type UpdateProgress } from "./autoUpdate";

const check = vi.fn();
vi.mock("@tauri-apps/plugin-updater", () => ({ check: () => check() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));

/** An update whose download replays `events` through the progress callback. */
function updateEmitting(events: DownloadEvent[]) {
  return {
    version: "1.2.0",
    currentVersion: "1.1.0",
    body: " notes ",
    downloadAndInstall: async (onEvent: (e: DownloadEvent) => void) => {
      for (const e of events) onEvent(e);
    },
  };
}

describe("checkForUpdate progress", () => {
  beforeEach(() => {
    check.mockReset();
    vi.spyOn(console, "info").mockImplementation(() => {});
  });

  it("reports the download from found through installing", async () => {
    check.mockResolvedValue(
      updateEmitting([
        { event: "Started", data: { contentLength: 300 } },
        { event: "Progress", data: { chunkLength: 100 } },
        { event: "Progress", data: { chunkLength: 200 } },
        { event: "Finished" },
      ]),
    );
    const seen: UpdateProgress[] = [];

    const result = await checkForUpdate((p) => seen.push(p));

    expect(result).toEqual({ kind: "staged", version: "1.2.0", notes: "notes" });
    expect(seen[0]).toEqual({ version: "1.2.0", phase: "downloading", received: 0, total: null });
    expect(seen[1]).toMatchObject({ phase: "downloading", total: 300 });
    // The burst of chunks is throttled, but the final report carries every byte.
    expect(seen.at(-1)).toEqual({
      version: "1.2.0",
      phase: "installing",
      received: 300,
      total: 300,
    });
    expect(seen.filter((p) => p.phase === "downloading" && p.received > 0).length).toBe(1);
  });

  it("reports nothing when already up to date", async () => {
    check.mockResolvedValue(null);
    const onProgress = vi.fn();

    expect(await checkForUpdate(onProgress)).toEqual({ kind: "uptodate" });
    expect(onProgress).not.toHaveBeenCalled();
  });
});
