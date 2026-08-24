import { describe, expect, it } from "vitest";
import type { DockerBuildEvent } from "@/api";
import { applyBuildEvent, type DockerBuildProgress, NEUTRAL_BUILD_RUNTIME } from "./sandbox";

const ev = (e: DockerBuildEvent) => e;
const building = (lastLine: string | null = null): DockerBuildProgress => ({
  status: "building",
  lastLine,
  error: null,
});

describe("applyBuildEvent", () => {
  it("opens a build entry keyed by the event's runtime", () => {
    const next = applyBuildEvent({}, ev({ phase: "started", runtime: "Podman" }));
    expect(next).toEqual({ Podman: building() });
  });

  it("routes an event without a runtime to the neutral key", () => {
    const next = applyBuildEvent({}, ev({ phase: "started" }));
    expect(Object.keys(next)).toEqual([NEUTRAL_BUILD_RUNTIME]);
    expect(next[NEUTRAL_BUILD_RUNTIME]).toEqual(building());
  });

  it("updates only its own key's tail on a line", () => {
    const before = { Docker: building("step 1"), Podman: building("pulling") };
    const next = applyBuildEvent(before, ev({ phase: "line", runtime: "Docker", line: "step 2" }));
    expect(next.Docker).toEqual(building("step 2"));
    expect(next.Podman).toBe(before.Podman);
  });

  it("materializes a building entry when a line arrives with no entry (reload mid-build)", () => {
    const next = applyBuildEvent({}, ev({ phase: "line", runtime: "Docker", line: "step 7" }));
    expect(next).toEqual({ Docker: building("step 7") });
  });

  it("keeps the previous tail when a line event carries no text", () => {
    const next = applyBuildEvent(
      { Docker: building("step 2") },
      ev({ phase: "line", runtime: "Docker" }),
    );
    expect(next.Docker).toEqual(building("step 2"));
  });

  it("deletes only its own key on finished", () => {
    const before = { Docker: building("step 2"), Podman: building("pulling") };
    const next = applyBuildEvent(before, ev({ phase: "finished", runtime: "Docker" }));
    expect(next).toEqual({ Podman: building("pulling") });
  });

  it("ignores a finished for a runtime with no entry", () => {
    const before = { Podman: building("pulling") };
    expect(applyBuildEvent(before, ev({ phase: "finished", runtime: "Docker" }))).toBe(before);
  });

  it("sets the error on its own key and leaves the other runtime building", () => {
    const before = { Podman: building("pulling") };
    const next = applyBuildEvent(
      before,
      ev({ phase: "failed", runtime: "Docker", error: "no space left" }),
    );
    expect(next.Docker).toEqual({ status: "failed", lastLine: null, error: "no space left" });
    expect(next.Podman).toBe(before.Podman);
  });

  it("falls back to generic copy when a failure carries no reason", () => {
    const next = applyBuildEvent({}, ev({ phase: "failed", runtime: "Podman" }));
    expect(next.Podman?.error).toBe("Image build failed");
  });

  it("clears a displayed failure when the build is retried", () => {
    const failed = applyBuildEvent(
      {},
      ev({ phase: "failed", runtime: "Podman", error: "no space left" }),
    );
    const retried = applyBuildEvent(failed, ev({ phase: "started", runtime: "Podman" }));
    expect(retried.Podman).toEqual(building());
  });

  it("never lets interleaved Docker and Podman lifecycles clear each other", () => {
    const events: DockerBuildEvent[] = [
      { phase: "started", runtime: "Docker" },
      { phase: "started", runtime: "Podman" },
      { phase: "line", runtime: "Docker", line: "docker step 1" },
      { phase: "line", runtime: "Podman", line: "podman step 1" },
      { phase: "finished", runtime: "Docker" },
      { phase: "line", runtime: "Podman", line: "podman step 2" },
    ];
    const state = events.reduce(applyBuildEvent, {} as Record<string, DockerBuildProgress>);
    // Docker's finish removed only Docker; Podman's build kept streaming.
    expect(state).toEqual({ Podman: building("podman step 2") });
  });

  it("keeps one runtime's failure visible while the other finishes", () => {
    const events: DockerBuildEvent[] = [
      { phase: "started", runtime: "Docker" },
      { phase: "started", runtime: "Podman" },
      { phase: "failed", runtime: "Docker", error: "build kaput" },
      { phase: "finished", runtime: "Podman" },
    ];
    const state = events.reduce(applyBuildEvent, {} as Record<string, DockerBuildProgress>);
    expect(state).toEqual({
      Docker: { status: "failed", lastLine: null, error: "build kaput" },
    });
  });

  it("does not mutate the state it is given", () => {
    const before = { Docker: building("step 1") };
    const snapshot = structuredClone(before);
    applyBuildEvent(before, ev({ phase: "failed", runtime: "Docker", error: "boom" }));
    applyBuildEvent(before, ev({ phase: "finished", runtime: "Docker" }));
    expect(before).toEqual(snapshot);
  });
});
