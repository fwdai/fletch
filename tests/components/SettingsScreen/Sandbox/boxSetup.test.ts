import { describe, expect, it } from "vitest";
import type { BoxImageStatus, BoxToolsStatus } from "@/api";
import { boxSetup } from "@/components/SettingsScreen/Sandbox/boxSetup";
import { formatClockTime } from "@/util/format";

const image: BoxImageStatus = {
  phase: "installed",
  version: "0.2.0",
  pinned_version: "0.2.0",
  url: "https://example.invalid/box-image",
};

const tools = (disk: Partial<BoxToolsStatus>): Record<string, BoxToolsStatus> => ({
  claude: { provider: "claude", version: "v2.1.314", phase: "ready", ...disk },
});

describe("boxSetup", () => {
  it("offers the tools build as Retry when the disk failed, with when a launch may try again", () => {
    const retryAt = Date.UTC(2026, 9, 30, 14, 5);
    const setup = boxSetup(
      null,
      image,
      tools({ phase: "failed", error: "npm exploded", retry_at: retryAt }),
    );
    expect(setup).toEqual({
      state: "error",
      text: `Claude Code tools failed: npm exploded · Retry now, or it's tried again on a launch after ${formatClockTime(retryAt)}`,
      retry: "tools",
      provider: "claude",
    });
  });

  it("says nothing of a next try once the backoff has passed", () => {
    const setup = boxSetup(null, image, tools({ phase: "failed", error: "npm exploded" }));
    expect(setup).toEqual({
      state: "error",
      text: "Claude Code tools failed: npm exploded",
      retry: "tools",
      provider: "claude",
    });
  });

  it("retries the image install when the image is what failed", () => {
    const setup = boxSetup(null, { ...image, phase: "unavailable", error: "offline" }, {});
    expect(setup).toEqual({ state: "error", text: "offline", retry: "image" });
  });

  it("reads a building disk as busy and a built one as ready", () => {
    expect(boxSetup(null, image, tools({ phase: "building" })).state).toBe("busy");
    expect(boxSetup(null, image, tools({})).state).toBe("ready");
  });
});
