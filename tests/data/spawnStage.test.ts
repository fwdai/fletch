import { describe, expect, it } from "vitest";
import type { SpawnStage } from "@/api";
import { spawnStageLabel } from "@/data/spawnStage";

describe("spawnStageLabel", () => {
  it("says what a fork's spawn is waiting on", () => {
    expect(spawnStageLabel({ stage: "carrying", detail: null })).toBe("Carrying over the code…");
    expect(spawnStageLabel({ stage: "summarizing", detail: null })).toBe(
      "Summarizing the conversation…",
    );
  });

  it("has nothing to say for a stage from a newer host", () => {
    const unknown = "teleporting" as SpawnStage;
    expect(spawnStageLabel({ stage: unknown, detail: null })).toBeUndefined();
  });
});
