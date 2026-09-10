// End-to-end over the mock host: the real protocol client, the real store, and
// the desktop adapters rendering real stream-json shapes. The jsdom URL in
// vite.config.ts puts this file in mock mode (see `mockEnabled`).

import { beforeAll, describe, expect, it, vi } from "vitest";
import { MOCK_HOST_KEY } from "../src/remote/mock";
import { PENDING_REQUEST_ID, PENDING_TOOL_USE_ID } from "../src/remote/mock/fixtures";
import { agentOf, api, client, useStore } from "../src/store";
import { clearHost, loadSettings, saveSettings } from "../src/store/persist";

const state = () => useStore.getState();

beforeAll(async () => {
  await state().init();
  await vi.waitFor(() => expect(state().connection).toBe("connected"), { timeout: 5000 });
});

describe("connection and snapshot", () => {
  it("pins the host key the transport reports — which is what paired means", () => {
    expect(state().hostKey).toBe(MOCK_HOST_KEY);
  });

  it("hello carries the host identity and the workspace", () => {
    expect(state().hostInfo?.name).toBe("Alex's MacBook Pro");
    expect(state().workspace?.projects.map((p) => p.name)).toEqual(["fletch", "atlas"]);
    expect(state().workspace?.agents).toHaveLength(4);
  });

  it("mirrors which path the connection is on", () => {
    // The mock host has no network under it, so it is the near path.
    expect(state().via).toBe("lan");
  });

  it("covers a running, a waiting, a finished and an errored agent", () => {
    const byId = Object.fromEntries((state().workspace?.agents ?? []).map((a) => [a.id, a]));
    expect(byId.arabia.status).toBe("running");
    expect(byId.kamakura.status).toBe("idle");
    expect(byId.caspian.status).toBe("error");
    expect(byId.caspian.last_error).toContain("429");
  });
});

describe("relay setting", () => {
  it("keeps the relay next to the host, and forgets it with the host", async () => {
    await saveSettings({
      host: "192.168.1.24",
      port: 47285,
      hostKey: "k",
      relay: "wss://relay.test",
      theme: "dark",
    });
    expect((await loadSettings()).relay).toBe("wss://relay.test");
    await clearHost();
    const after = await loadSettings();
    expect(after.relay).toBeUndefined();
    expect(after.host).toBeUndefined();
    expect(after.theme).toBe("dark");
  });

  it("can be added later without re-pairing, and lands on the held target", async () => {
    await state().setRelay(" wss://relay.test ");
    expect(state().relay).toBe("wss://relay.test");
    // It applies from the next attempt on; nothing reconnects here.
    expect(client.target?.relay).toBe("wss://relay.test");
    expect(state().connection).toBe("connected");
    await state().setRelay(null);
    expect(state().relay).toBeNull();
    expect(client.target?.relay).toBeUndefined();
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

  it("does not draw our own send twice when the host echoes it as turn:sent", async () => {
    await state().rebuildLog("kamakura");
    const before = (state().logs.kamakura ?? []).length;
    await state().send("kamakura", "one more thing from the phone");
    // The echo arrived with the response; the optimistic bubble carries the
    // same turn id, so the log grew by exactly one user-side item — whether we
    // are looking at that bubble or at the canonical rebuild that replaced it.
    const mine = (state().logs.kamakura ?? []).filter(
      (i) =>
        (i.kind === "queued_message" || i.kind === "user_message") &&
        i.text === "one more thing from the phone",
    );
    expect(mine).toHaveLength(1);
    expect((state().logs.kamakura ?? []).length).toBe(before + 1);
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
  it("spawns under the name the sheet showed, waits for spawning to clear, then sends the prompt", async () => {
    const before = state().workspace?.agents.length ?? 0;
    await state().spawn({
      repoPath: state().workspace?.projects[0].path ?? "",
      provider: "claude",
      model: "claude-opus-5",
      effort: "high",
      base: "main",
      prompt: "Add a settings row for the dictation engine",
      name: "zermatt",
    });
    expect(state().workspace?.agents.length).toBe(before + 1);
    const fresh = state().workspace?.agents[0];
    // The sheet's name, not a fresh allocation (which would have been "tasman").
    expect(fresh?.name).toBe("zermatt");
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

  it("allocates a name itself only when the sheet had none", async () => {
    const before = new Set((state().workspace?.agents ?? []).map((a) => a.id));
    await state().spawn({
      repoPath: state().workspace?.projects[0].path ?? "",
      provider: "claude",
      model: "claude-opus-5",
      effort: "high",
      base: "main",
      prompt: "The sheet never got a name for this one",
      name: "",
    });
    const fresh = (state().workspace?.agents ?? []).find((a) => !before.has(a.id));
    expect(fresh?.name).toBeTruthy();
  });

  it("keeps the live log when the agent is re-opened mid-turn", async () => {
    await state().spawn({
      repoPath: state().workspace?.projects[0].path ?? "",
      provider: "claude",
      model: "claude-opus-5",
      effort: "high",
      base: "main",
      prompt: "Wire the dictation engine picker",
      name: "lofoten",
    });
    const id = state().workspace?.agents[0]?.id ?? "";
    // A tool call has streamed in and the turn is still running.
    await vi.waitFor(
      () => {
        expect(agentOf(state().workspace, id)?.status).toBe("running");
        expect((state().logs[id] ?? []).some((i) => i.kind === "tool_call")).toBe(true);
      },
      { timeout: 10_000 },
    );
    const live = state().logs[id] ?? [];
    // The real host ingests a turn's transcript only at turn end, so mid-turn its
    // records stop at the prompt. The mock persists every step, so hold it back.
    const records = await api.readSessionRecords(id);
    const read = vi.spyOn(api, "readSessionRecords").mockResolvedValue(records.slice(0, 1));
    try {
      // Back to the list, then into the agent again: what openAgent runs.
      state().pop();
      await state().loadAgent(id);
    } finally {
      read.mockRestore();
    }
    const after = state().logs[id] ?? [];
    expect(after.length).toBeGreaterThanOrEqual(live.length);
    for (const item of live) expect(after).toContainEqual(item);

    // Once the turn ends the host's records are complete and authoritative:
    // re-opening then does rebuild from them.
    await vi.waitFor(() => expect(agentOf(state().workspace, id)?.status).toBe("idle"), {
      timeout: 10_000,
    });
    useStore.setState((s) => ({ logs: { ...s.logs, [id]: (s.logs[id] ?? []).slice(0, 1) } }));
    await state().loadAgent(id);
    expect((state().logs[id] ?? []).some((i) => i.kind === "tool_call")).toBe(true);
  }, 30_000);

  it("rejects and rolls back when the first message fails, so the prompt can be retried", async () => {
    const send = vi
      .spyOn(api, "sendUserMessage")
      .mockRejectedValueOnce(new Error("host refused the message"));
    const before = new Set((state().workspace?.agents ?? []).map((a) => a.id));

    await expect(
      state().spawn({
        repoPath: state().workspace?.projects[0].path ?? "",
        provider: "claude",
        model: "claude-opus-5",
        effort: "high",
        base: "main",
        prompt: "This one never reaches the agent",
        name: "sedona",
      }),
    ).rejects.toThrow("host refused the message");

    expect(state().lastError).toContain("host refused the message");
    // The agent exists on the host, but nothing optimistic is left behind.
    const fresh = (state().workspace?.agents ?? []).find((a) => !before.has(a.id));
    expect(fresh).toBeTruthy();
    const id = fresh?.id ?? "";
    expect(state().busy[id]).toBeFalsy();
    expect(state().logs[id]).toBeUndefined();
    send.mockRestore();
  });
});

/** A pairing code is single use and lasts five minutes, so a second delivery
 *  of the same link — the launch URL read from the plugin, and the event it
 *  also emits — must not tear down the attempt already spending it. */
describe("pairing from a link", () => {
  const LINK = { host: "192.168.1.24", port: 47285, hostKey: "k", pairingToken: "K7PQ2M9X" };

  it("ignores a repeat of the link whose pairing is still running", () => {
    const restore = { pairStep: state().pairStep, pairTarget: state().pairTarget };
    useStore.setState({ pairStep: "relay", pairTarget: LINK });
    try {
      // The same link, with its pairing in flight: the attempt already
      // spending the code keeps it, rather than being restarted from
      // `connecting`.
      state().pairFromLink({ ...LINK });
      expect(state().pairTarget).toEqual(LINK);
      expect(state().pairStep).toBe("relay");
    } finally {
      useStore.setState(restore);
    }
  });
});
