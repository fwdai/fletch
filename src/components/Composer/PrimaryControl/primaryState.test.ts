import { describe, expect, it } from "vitest";
import { formatClock, type PrimaryInputs, primaryState } from "./primaryState";

const idle: PrimaryInputs = {
  sttError: false,
  dictation: "idle",
  agentRunning: false,
  hasDraft: false,
  micDenied: false,
};

describe("primaryState", () => {
  it("rests on the mic when nothing is happening", () => {
    expect(primaryState(idle)).toBe("empty");
  });

  it("offers send once there is a draft", () => {
    expect(primaryState({ ...idle, hasDraft: true })).toBe("draft");
  });

  it("shows the live mic from the click, not from the first audio", () => {
    expect(primaryState({ ...idle, dictation: "starting" })).toBe("listening");
    expect(primaryState({ ...idle, dictation: "listening" })).toBe("listening");
  });

  it("holds transcribing over a draft and a run", () => {
    expect(
      primaryState({ ...idle, dictation: "transcribing", hasDraft: true, agentRunning: true }),
    ).toBe("transcribing");
  });

  it("is the stop button while the agent runs, even with a draft waiting", () => {
    expect(primaryState({ ...idle, agentRunning: true })).toBe("running");
    expect(primaryState({ ...idle, agentRunning: true, hasDraft: true })).toBe("running");
  });

  it("puts a failure above everything", () => {
    expect(
      primaryState({ ...idle, sttError: true, dictation: "listening", agentRunning: true }),
    ).toBe("error");
  });

  it("only replaces empty when the mic is denied", () => {
    expect(primaryState({ ...idle, micDenied: true })).toBe("unavailable");
    expect(primaryState({ ...idle, micDenied: true, hasDraft: true })).toBe("draft");
    expect(primaryState({ ...idle, micDenied: true, agentRunning: true })).toBe("running");
  });
});

describe("formatClock", () => {
  it("renders minutes and zero-padded seconds", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(7.9)).toBe("0:07");
    expect(formatClock(65)).toBe("1:05");
    expect(formatClock(-3)).toBe("0:00");
  });
});
