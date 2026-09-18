// End-to-end over the mock host: the real protocol client, the real store, and
// the desktop adapters rendering real stream-json shapes. The jsdom URL in
// vite.config.ts puts this file in mock mode (see `mockEnabled`).

import { ROADMAP_PM_PURPOSE } from "@desktop/api/types/agent";
import { PROJECT_MANAGER_PRESET } from "@desktop/starterPack/presets";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { MOCK_HOST_KEY } from "../src/remote/mock";
import {
  PENDING_REQUEST_ID,
  PENDING_TOOL_USE_ID,
  PM_CUSTOM_AGENT_ID,
} from "../src/remote/mock/fixtures";
import { agentOf, api, client, projectOf, useStore } from "../src/store";
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
    await vi.waitFor(() => expect(agentOf(state(), "arabia")?.status).toBe("idle"), {
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

  it("sends attachments alone, carries them on the bubble, and the echo still draws once", async () => {
    await state().rebuildLog("kamakura");
    const before = (state().logs.kamakura ?? []).length;
    const send = vi.spyOn(api, "sendUserMessage");
    const path = "/Users/alex/Library/Application Support/sh.fletch.app/attachments/up-1/shot.png";
    try {
      await state().send("kamakura", "", [path]);
      expect(send).toHaveBeenCalledWith("kamakura", expect.any(String), "", [path]);
    } finally {
      send.mockRestore();
    }
    const mine = (state().logs.kamakura ?? []).filter(
      (i) =>
        (i.kind === "queued_message" || i.kind === "user_message") && i.attachments?.includes(path),
    );
    expect(mine).toHaveLength(1);
    expect((state().logs.kamakura ?? []).length).toBe(before + 1);
  });

  it("a send with neither text nor attachments is a no-op", async () => {
    const send = vi.spyOn(api, "sendUserMessage");
    try {
      await state().send("kamakura", "   ", []);
      expect(send).not.toHaveBeenCalled();
    } finally {
      send.mockRestore();
    }
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
  it("loads git state and the PR lazily per agent", async () => {
    await state().loadGit("kamakura");
    expect(state().gitStates.kamakura?.branch).toBe("fix/dictation-followups");
    expect(state().prStates.kamakura?.number).toBe(642);
    expect(state().prChecks.kamakura?.passed).toBe(14);
    await state().loadGit("arabia");
    expect(state().gitStates.arabia?.files).toHaveLength(3);
  });

  it("polls working-tree stats for the whole fleet, and replaces them wholesale", async () => {
    await state().loadShortstats();
    expect(state().shortstats.arabia?.additions).toBe(136);
    // kamakura's tree is clean, so the host never names it.
    expect(state().shortstats.kamakura).toBeUndefined();
    // A later poll that drops an agent drops its numbers with it, rather than
    // leaving a stale count on a row whose work has been committed away.
    const spy = vi
      .spyOn(api, "getAllShortstats")
      .mockResolvedValue({ pamukkale: { additions: 42, deletions: 17, file_count: 1 } });
    await state().loadShortstats();
    expect(state().shortstats.arabia).toBeUndefined();
    expect(state().shortstats.pamukkale?.additions).toBe(42);
    spy.mockRestore();
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

describe("answering a gated publish", () => {
  const request = {
    id: "pub-1",
    agent_id: "arabia",
    op: "git_push",
    detail: "push fix/onboarding-flicker",
  };

  it("gates on what the host reported it answers, not on its version", () => {
    expect(state().protocol?.version).toBe(2);
    expect(state().hostSupports("answer_publish_approval")).toBe(true);
    expect(state().hostSupports("open_agent_shell")).toBe(false);
  });

  it("holds the prompt until it is answered, then drops it", async () => {
    state().receivePublishApproval(request);
    expect(state().pendingPublishApprovals).toContainEqual(request);
    await state().answerPublishApproval(request.id, true);
    expect(state().pendingPublishApprovals).toHaveLength(0);
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

  it("carries the sheet's attachments on the first message", async () => {
    const send = vi.spyOn(api, "sendUserMessage");
    const path = "/Users/alex/Library/Application Support/sh.fletch.app/attachments/up-2/spec.pdf";
    try {
      await state().spawn({
        repoPath: state().workspace?.projects[0].path ?? "",
        provider: "claude",
        model: "claude-opus-5",
        effort: "high",
        base: "main",
        prompt: "Implement what the attached spec describes",
        attachments: [path],
        name: "skye",
      });
      expect(send).toHaveBeenCalledWith(
        "skye",
        expect.any(String),
        "Implement what the attached spec describes",
        [path],
      );
    } finally {
      send.mockRestore();
    }
    expect(state().logs.skye?.[0]).toMatchObject({ kind: "user_message", attachments: [path] });
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
        expect(agentOf(state(), id)?.status).toBe("running");
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
    await vi.waitFor(() => expect(agentOf(state(), id)?.status).toBe("idle"), {
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

/** The optimistic `busy` flag is set on send and cleared by live events. A
 *  backgrounded webview or a dropped socket misses those, so every fresh
 *  snapshot reconciles it — otherwise the Changes tab sits on "Agent is busy…"
 *  for an agent that finished while the phone was away. */
describe("optimistic busy flag", () => {
  it("clears against a fresh snapshot when the turn ended off-socket", async () => {
    useStore.setState((s) => ({ busy: { ...s.busy, caspian: true } }));
    await state().refreshWorkspace();
    expect(agentOf(state(), "caspian")?.status).not.toBe("running");
    expect(state().busy.caspian).toBe(false);
  });

  it("leaves a send that is still in flight alone", async () => {
    let release = () => {};
    const send = vi.spyOn(api, "sendUserMessage").mockImplementation(
      () =>
        new Promise<boolean>((resolve) => {
          release = () => resolve(false);
        }),
    );
    try {
      const pending = state().send("caspian", "hold this one open");
      expect(state().busy.caspian).toBe(true);
      // The host cannot have flipped the agent to running yet — the snapshot
      // is older than the tap, so it must not clear the flag.
      await state().refreshWorkspace();
      expect(state().busy.caspian).toBe(true);
      release();
      await pending;
    } finally {
      send.mockRestore();
    }
  });
});

/** A planning chat is an ordinary agent record tagged with a purpose, which is
 *  what keeps it out of the workspace snapshot. It therefore lives in its own
 *  per-project registry, and everything that addresses an agent by id reaches it
 *  through `agentOf`'s fallback. */
describe("planning chats", () => {
  const projectId = "prj-fletch";
  const repoPath = () => useStore.getState().workspace?.projects[0].path ?? "";

  it("is offered only by a host that lists them", () => {
    expect(state().hostSupports("list_project_chats")).toBe(true);
  });

  it("lists the chats the snapshot holds back, and resolves them like any agent", async () => {
    expect((state().workspace?.agents ?? []).some((a) => a.id === "sakura")).toBe(false);
    await state().loadChats(projectId);
    expect(state().chats[projectId]?.map((c) => c.id)).toContain("sakura");
    // The fallback is what makes the agent screen, the composer and the event
    // handlers work for a chat without knowing it is one.
    expect(agentOf(state(), "sakura")?.purpose).toBe(ROADMAP_PM_PURPOSE);
    expect(projectOf(state(), "sakura")?.name).toBe("fletch");
  });

  it("spawns under the Mac's Project Manager, registers the chat, then sends the idea", async () => {
    const spawn = vi.spyOn(api, "spawnAgent");
    try {
      await state().startPlanningChat({
        projectId,
        prompt: "An offline queue for messages typed underground",
      });
      expect(spawn).toHaveBeenCalledWith(
        repoPath(),
        PROJECT_MANAGER_PRESET.base,
        expect.any(String),
        PROJECT_MANAGER_PRESET.effort,
        PROJECT_MANAGER_PRESET.model,
        "main",
        {
          instructions: PROJECT_MANAGER_PRESET.instructions,
          customAgentId: PM_CUSTOM_AGENT_ID,
          purpose: ROADMAP_PM_PURPOSE,
        },
      );
    } finally {
      spawn.mockRestore();
    }
    const chat = state().chats[projectId]?.[0];
    expect(chat?.purpose).toBe(ROADMAP_PM_PURPOSE);
    // The chat is prepended, and it is not in the snapshot the host answers.
    expect(state().chats[projectId]?.map((c) => c.id)).toContain("sakura");
    expect((state().workspace?.agents ?? []).some((a) => a.id === chat?.id)).toBe(false);
    // The record came back `spawning`; only the live `agent:status` can have
    // cleared it, and for a chat that patch lands in the registry — which is
    // also what let `waitForSpawn` release the first message below.
    expect(chat?.status).not.toBe("spawning");
    await vi.waitFor(
      () =>
        expect(
          (state().logs[chat?.id ?? ""] ?? []).some(
            (i) => i.kind === "user_message" && i.text.includes("offline queue"),
          ),
        ).toBe(true),
      { timeout: 5000 },
    );
  });

  it("falls back to the bundled preset when the Mac's library has no Project Manager", async () => {
    const list = vi.spyOn(api, "listCustomAgents").mockResolvedValue([]);
    const spawn = vi.spyOn(api, "spawnAgent");
    try {
      await state().startPlanningChat({ projectId, prompt: "Ship the widget" });
      expect(spawn).toHaveBeenCalledWith(
        repoPath(),
        PROJECT_MANAGER_PRESET.base,
        expect.any(String),
        PROJECT_MANAGER_PRESET.effort,
        PROJECT_MANAGER_PRESET.model,
        "main",
        expect.objectContaining({
          // Nothing is seeded from the phone: the brief travels with the spawn.
          customAgentId: null,
          instructions: PROJECT_MANAGER_PRESET.instructions,
        }),
      );
    } finally {
      spawn.mockRestore();
      list.mockRestore();
    }
  });

  it("patches a chat record in place, and ignores an id it does not hold", () => {
    state().patchChat("sakura", { task: "Offline queue, sliced" });
    expect(state().chats[projectId]?.find((c) => c.id === "sakura")?.task).toBe(
      "Offline queue, sliced",
    );
    const before = state().chats;
    state().patchChat("arabia", { task: "not a chat" });
    expect(state().chats).toBe(before);
  });

  it("resolves a chat from its id alone — all a tapped notification carries", async () => {
    // A cold launch: the snapshot never named the chat and nothing has listed
    // it, so the agent screen has an id and an empty registry behind it.
    useStore.setState({ chats: {} });
    await state().ensureAgent("sakura");
    expect(agentOf(state(), "sakura")?.purpose).toBe(ROADMAP_PM_PURPOSE);
    // An id the host does not know registers nothing, rather than a placeholder.
    const before = state().chats;
    await state().ensureAgent("no-such-agent");
    expect(state().chats).toBe(before);
  });
});

/** A ticket the PM proposed is a real board row parked at `proposed` — a ghost
 *  nobody has ruled on. The phone holds those per project and draws them in the
 *  planning chat that raised them, so the whole surface is: read the board and
 *  keep the ghosts, fold the live rows, and rule. */
describe("PM proposals", () => {
  const projectId = "prj-fletch";
  const ghosts = () => state().proposals[projectId] ?? [];
  const ghost = (code: string) => ghosts().find((i) => i.code === code);

  it("keeps only the ghosts off the board, newest first", async () => {
    await state().loadProposals(projectId);
    // FLT-198 is already open — an item, not a question.
    expect(ghosts().map((i) => i.code)).toEqual(["FLT-207", "FLT-209"]);
  });

  it("accepts as a conditional transition, and the card goes with the ruling", async () => {
    await state().loadProposals(projectId);
    const item = ghost("FLT-207");
    if (!item) throw new Error("no ghost to rule on");
    const update = vi
      .spyOn(api, "roadmapUpdateItem")
      .mockResolvedValue({ applied: true, item: { ...item, status: "queued" } });
    try {
      await state().acceptProposal(item, true);
      // Exactly the desktop board's accept: `proposed → open`, conditional on
      // the row still being a proposal, with `queue` handing it straight on.
      expect(update).toHaveBeenCalledWith(item.id, { status: "open" }, "proposed", true);
    } finally {
      update.mockRestore();
    }
    expect(ghost("FLT-207")).toBeUndefined();
  });

  it("discards by deleting the row — it never made the board", async () => {
    await state().loadProposals(projectId);
    const item = ghost("FLT-209");
    if (!item) throw new Error("no ghost to rule on");
    const discard = vi
      .spyOn(api, "roadmapDiscardProposal")
      .mockResolvedValue({ applied: true, item: null });
    try {
      await state().discardProposal(item);
      expect(discard).toHaveBeenCalledWith(item.id);
    } finally {
      discard.mockRestore();
    }
    expect(ghost("FLT-209")).toBeUndefined();
  });

  it("deletes nothing when the row was accepted first, and still takes the card down", async () => {
    await state().loadProposals(projectId);
    const item = ghost("FLT-209");
    if (!item) throw new Error("no ghost to rule on");
    // What the host answers for a row that has moved on: the discard is refused
    // and the row comes back as it really is.
    const discard = vi
      .spyOn(api, "roadmapDiscardProposal")
      .mockResolvedValue({ applied: false, item: { ...item, status: "queued" } });
    try {
      await state().discardProposal(item);
    } finally {
      discard.mockRestore();
    }
    // The card goes — a queued row is not a question this chat is waiting on —
    // but the item someone accepted is still on the board.
    expect(ghost("FLT-209")).toBeUndefined();
    expect((await api.roadmapListItems(projectId)).some((i) => i.id === item.id)).toBe(true);
  });

  it("replaces the card when a `roadmap:item` says the row is still proposed", async () => {
    await state().loadProposals(projectId);
    // Written on the host, so only the event can carry it here — a PM revising
    // its own suggestion must replace the card rather than stack a second one.
    await api.roadmapUpdateItem("itm-offline-queue", { title: "Queue sends made underground" });
    await vi.waitFor(() => expect(ghost("FLT-207")?.title).toBe("Queue sends made underground"));
    expect(ghosts()).toHaveLength(2);
  });

  it("takes the card down when a `roadmap:item` says the row was ruled on", async () => {
    await state().loadProposals(projectId);
    await api.roadmapUpdateItem("itm-offline-queue", { status: "open" });
    await vi.waitFor(() => expect(ghost("FLT-207")).toBeUndefined());
    expect(ghosts().map((i) => i.code)).toEqual(["FLT-209"]);
  });

  it("takes the card down on `roadmap:item-deleted`, which carries the bare id", async () => {
    await state().loadProposals(projectId);
    await api.roadmapDiscardProposal("itm-tunnel-banner");
    await vi.waitFor(() => expect(ghosts()).toHaveLength(0));
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
