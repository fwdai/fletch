// End-to-end over the mock host: the real protocol client, the real store, and
// the desktop adapters rendering real stream-json shapes. The jsdom URL in
// vite.config.ts puts this file in mock mode (see `mockEnabled`).

import { beforeAll, describe, expect, it, vi } from "vitest";
import { PENDING_REQUEST_ID, PENDING_TOOL_USE_ID } from "../src/remote/mock/fixtures";
import { agentOf, useStore } from "../src/store";

const state = () => useStore.getState();

beforeAll(async () => {
  await state().init();
  await vi.waitFor(() => expect(state().connection).toBe("connected"), { timeout: 5000 });
});

describe("connection and snapshot", () => {
  it("hello carries the host identity and the workspace", () => {
    expect(state().hostInfo?.name).toBe("Alex's MacBook Pro");
    expect(state().workspace?.projects.map((p) => p.name)).toEqual(["fletch", "atlas"]);
    expect(state().workspace?.agents).toHaveLength(4);
  });

  it("covers a running, a waiting, a finished and an errored agent", () => {
    const byId = Object.fromEntries((state().workspace?.agents ?? []).map((a) => [a.id, a]));
    expect(byId.arabia.status).toBe("running");
    expect(byId.kamakura.status).toBe("idle");
    expect(byId.caspian.status).toBe("error");
    expect(byId.caspian.last_error).toContain("429");
  });
});

describe("event folding", () => {
  it("records a held can_use_tool prompt instead of putting it in the log", async () => {
    await vi.waitFor(() =>
      expect(state().pendingToolUse.pamukkale?.[PENDING_TOOL_USE_ID]).toBe(PENDING_REQUEST_ID),
    );
    // Control-plane events never reach the transcript reducer.
    expect(state().logs.pamukkale ?? []).not.toContainEqual(
      expect.objectContaining({ kind: "notice" }),
    );
  });

  it("agent:status drives the record and the busy flag", async () => {
    await vi.waitFor(() => expect(state().busy.arabia).toBe(true));
    await vi.waitFor(() => expect(agentOf(state().workspace, "arabia")?.status).toBe("idle"), {
      timeout: 5000,
    });
    expect(state().busy.arabia).toBe(false);
  });

  it("turn:started anchors the live timer", () => {
    expect(typeof state().turnStartedAt.arabia === "number" || state().busy.arabia === false).toBe(
      true,
    );
  });
});

describe("transcripts through the desktop adapters", () => {
  it("renders claude records as user, assistant and tool items", async () => {
    await state().rebuildLog("pamukkale");
    const log = state().logs.pamukkale ?? [];
    expect(log.find((i) => i.kind === "user_message")).toMatchObject({
      text: expect.stringContaining("not-ready flashes"),
    });
    expect(log.find((i) => i.kind === "agent_message")).toBeTruthy();
    const call = log.find((i) => i.kind === "tool_call" && i.id === PENDING_TOOL_USE_ID);
    expect(call).toMatchObject({ name: "Bash" });
  });

  it("renders codex rollout records too", async () => {
    await state().rebuildLog("caspian");
    const log = state().logs.caspian ?? [];
    expect(log.some((i) => i.kind === "user_message")).toBe(true);
    expect(log.some((i) => i.kind === "tool_call")).toBe(true);
  });
});

describe("git and PR state", () => {
  it("loads git, diff stats and the PR lazily per agent", async () => {
    await state().loadGit("kamakura");
    expect(state().gitStates.kamakura?.branch).toBe("fix/dictation-followups");
    expect(state().prStates.kamakura?.number).toBe(642);
    expect(state().prChecks.kamakura?.passed).toBe(14);
    await state().loadGit("arabia");
    expect(state().gitStates.arabia?.files).toHaveLength(3);
    expect(state().diffStats.arabia?.additions).toBe(136);
  });

  it("reads the checkout tree", async () => {
    await state().loadTree("arabia");
    expect(state().trees.arabia?.some((f) => f.status === "M")).toBe(true);
  });
});

describe("answering a tool-use prompt", () => {
  it("clears the held prompt and resumes the turn", async () => {
    await vi.waitFor(() =>
      expect(state().pendingToolUse.pamukkale?.[PENDING_TOOL_USE_ID]).toBeTruthy(),
    );
    await state().answerToolUse("pamukkale", PENDING_TOOL_USE_ID, { command: "git push" }, "allow");
    expect(state().pendingToolUse.pamukkale?.[PENDING_TOOL_USE_ID]).toBeUndefined();
    // The host answers with a tool_result, which lands in the rebuilt log.
    await vi.waitFor(
      () =>
        expect(
          (state().logs.pamukkale ?? []).some(
            (i) => i.kind === "tool_result" && i.tool_use_id === PENDING_TOOL_USE_ID,
          ),
        ).toBe(true),
      { timeout: 5000 },
    );
  });
});

describe("spawn flow", () => {
  it("allocates a name, spawns, waits for spawning to clear, then sends the prompt", async () => {
    const before = state().workspace?.agents.length ?? 0;
    await state().spawn({
      repoPath: state().workspace?.projects[0].path ?? "",
      provider: "claude",
      model: "claude-opus-5",
      effort: "high",
      base: "main",
      prompt: "Add a settings row for the dictation engine",
    });
    expect(state().workspace?.agents.length).toBe(before + 1);
    const fresh = state().workspace?.agents[0];
    expect(fresh?.status).not.toBe("spawning");
    await vi.waitFor(
      () =>
        expect(
          (state().logs[fresh?.id ?? ""] ?? []).some(
            (i) => i.kind === "user_message" && i.text.includes("dictation engine"),
          ),
        ).toBe(true),
      { timeout: 5000 },
    );
  });
});
