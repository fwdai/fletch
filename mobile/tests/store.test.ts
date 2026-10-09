// End-to-end over the mock host: the real protocol client, the real store, and
// the desktop adapters rendering real stream-json shapes. The jsdom URL in
// vite.config.ts puts this file in mock mode (see `mockEnabled`).

import { type AgentManagedEvent, ROADMAP_PM_PURPOSE } from "@desktop/api/types/agent";
import type { AutopilotLogEntry } from "@desktop/api/types/git";
import type { LiveTurn } from "@desktop/api/types/session";
import { PROJECT_MANAGER_PRESET } from "@desktop/starterPack/presets";
import { beforeAll, describe, expect, it, type MockInstance, vi } from "vitest";
import type { ChatItem } from "../src/adapters";
import { MOCK_HOST_KEY } from "../src/remote/mock";
import {
  prChecks as fixturePrChecks,
  prStates as fixturePrStates,
  relay as fixtureRelay,
  PENDING_REQUEST_ID,
  PENDING_TOOL_USE_ID,
  PM_CUSTOM_AGENT_ID,
} from "../src/remote/mock/fixtures";
import { agentOf, api, client, projectOf, useStore } from "../src/store";
import { isReplayed, replayLiveTurn } from "../src/store/liveTurn";
import { clearHost, loadSettings, saveSettings } from "../src/store/persist";
import { pastHistory } from "../src/store/transcript";

const state = () => useStore.getState();

/** Deliver a host event frame to the store's listeners, as the socket would:
 *  for the events the mock host never emits on its own. */
const hostEvent = (event: string, payload: unknown) =>
  (
    client as unknown as { dispatchEvent(frame: { event: string; payload: unknown }): void }
  ).dispatchEvent({ event, payload });

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

  it("follows the relay the host answers, with no setting of its own", () => {
    // The mock was dialled with no relay; the handshake supplied it.
    expect(state().relay).toBe(fixtureRelay);
    expect(client.target?.relay).toBe(fixtureRelay);
    expect("setRelay" in state()).toBe(false);
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

  it("agent:status drives the record and discharges this device's send", async () => {
    await vi.waitFor(() => expect(agentOf(state(), "arabia")?.status).toBe("running"), {
      timeout: 5000,
    });
    // A send of ours: the host answers it with `running`, or failing that the
    // turn's `idle` — a status is what discharges it, never a transcript event.
    useStore.setState((s) => ({ sending: { ...s.sending, arabia: true } }));
    await vi.waitFor(() => expect(agentOf(state(), "arabia")?.status).toBe("idle"), {
      timeout: 5000,
    });
    expect(state().sending.arabia).toBeUndefined();
  });

  it("turn:started anchors the live timer", () => {
    expect(
      typeof state().turnStartedAt.arabia === "number" ||
        agentOf(state(), "arabia")?.status !== "running",
    ).toBe(true);
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

/** The replay against the stream still running under it. Pure, so the exact
 *  interleavings are pinned down rather than hoped for in a timing test. */
describe("replaying the running turn", () => {
  const text = (n: number) => ({
    type: "assistant",
    message: { role: "assistant", content: [{ type: "text", text: `message ${n}` }] },
  });
  const texts = (items: ChatItem[]) =>
    items.flatMap((i) => (i.kind === "agent_message" ? [i.text] : []));

  it("folds a frame that arrived after the snapshot, once, and skips one it already holds", () => {
    const id = "zanskar";
    const live: LiveTurn = { events: [text(1), text(2)], dropped: 0, next_seq: 2 };
    // Both reached the phone while the snapshot was in flight: the first is
    // in the snapshot (its frame simply overtook the response), the second
    // happened after the host took it.
    const late: AgentManagedEvent[] = [
      { agent_id: id, event: text(2), seq: 1 },
      { agent_id: id, event: text(3), seq: 2 },
    ];
    const patch = replayLiveTurn(state(), id, [], live, late);
    expect(texts(patch.logs?.[id] ?? [])).toEqual(["message 1", "message 2", "message 3"]);
    expect(patch.liveSeq?.[id]).toBe(2);
  });

  it("then drops a live frame the snapshot covered and keeps the next one", () => {
    const liveSeq = { zanskar: 2 };
    expect(isReplayed(liveSeq, { agent_id: "zanskar", event: text(2), seq: 1 })).toBe(true);
    expect(isReplayed(liveSeq, { agent_id: "zanskar", event: text(3), seq: 2 })).toBe(false);
    // Another agent's frames, and a host that numbers nothing, are untouched.
    expect(isReplayed(liveSeq, { agent_id: "other", event: text(1), seq: 0 })).toBe(false);
    expect(isReplayed(liveSeq, { agent_id: "zanskar", event: text(1) })).toBe(false);
  });

  it("says so when the host dropped the head of the turn", () => {
    const patch = replayLiveTurn(state(), "zanskar", [], {
      events: [text(9)],
      dropped: 8,
      next_seq: 9,
    });
    const log = patch.logs?.zanskar ?? [];
    expect(log[0]).toMatchObject({ kind: "notice", text: expect.stringContaining("8 earlier") });
    expect(texts(log)).toEqual(["message 9"]);
  });
});

/** Loading an older page re-renders the history whole and keeps what the log
 *  held past it, without drawing a turn's bubble twice. */
describe("the log past its history", () => {
  const rendered: ChatItem[] = [
    { kind: "user_message", text: "q1", turnId: "t1", recordSeq: 1, recordSession: "s" },
    // A bubble the merge drew from a row, after the last record.
    { kind: "user_message", text: "stopped", turnId: "t2", undelivered: "interrupted" },
  ];
  const live: ChatItem[] = [
    { kind: "user_message", text: "running", turnId: "t3" },
    { kind: "agent_message", text: "working", streaming: true },
  ];

  it("is what follows the last record-rendered item, less the bubbles a re-render draws", () => {
    const fresh: ChatItem[] = [{ kind: "user_message", text: "q0", recordSeq: 1 }, ...rendered];
    expect(pastHistory([...rendered, ...live], fresh)).toEqual(live);
  });

  it("is the whole log when nothing in it came from a record", () => {
    expect(pastHistory(live, [])).toEqual(live);
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

  it("re-reads a turn-end transcript only for an agent whose screen is open", async () => {
    const read = vi.spyOn(api, "readSessionPage");
    const home = { key: Date.now(), screen: "home" as const, props: {}, phase: "idle" as const };
    try {
      useStore.setState({ nav: [home] });
      hostEvent("session:records-appended", { agent_id: "pamukkale" });
      await new Promise((r) => setTimeout(r, 20));
      expect(read).not.toHaveBeenCalled();

      state().openAgent("pamukkale");
      await vi.waitFor(() => expect(read).toHaveBeenCalledWith("pamukkale"));
      read.mockClear();
      hostEvent("session:records-appended", { agent_id: "pamukkale" });
      await vi.waitFor(() => expect(read).toHaveBeenCalledWith("pamukkale"));
    } finally {
      read.mockRestore();
      useStore.setState({ nav: [home] });
    }
  });
});

/** What lets the chat tell "still loading" and "failed" apart from a log it
 *  has: a chat always has messages, so an empty screen is never the answer. */
describe("history load state", () => {
  it("is loading from the moment the read starts, and ready once it lands", async () => {
    useStore.setState((s) => {
      const logLoads = { ...s.logLoads };
      delete logLoads.pamukkale;
      return { logLoads };
    });
    const read = state().rebuildLog("pamukkale");
    expect(state().logLoads.pamukkale).toEqual({ status: "loading" });
    await read;
    expect(state().logLoads.pamukkale).toEqual({ status: "ready" });
  });

  it("records a failed read on the chat, not in lastError, and rethrows", async () => {
    const read = vi.spyOn(api, "readSessionPage").mockRejectedValue(new Error("timed out"));
    useStore.setState({ lastError: null });
    try {
      await expect(state().rebuildLog("pamukkale")).rejects.toThrow("timed out");
    } finally {
      read.mockRestore();
    }
    expect(state().logLoads.pamukkale).toEqual({ status: "error", error: "timed out" });
    expect(state().lastError).toBeNull();
  });

  it("lets only the newest read say how loading went", async () => {
    let fail!: (e: Error) => void;
    const real = api.readSessionPage;
    const read = vi
      .spyOn(api, "readSessionPage")
      .mockImplementationOnce(() => new Promise((_, reject) => (fail = reject)))
      .mockImplementation((...args) => real.apply(api, args));
    try {
      const older = state().rebuildLog("pamukkale");
      await state().rebuildLog("pamukkale");
      fail(new Error("socket closed"));
      await expect(older).rejects.toThrow("socket closed");
    } finally {
      read.mockRestore();
    }
    expect(state().logLoads.pamukkale).toEqual({ status: "ready" });
  });

  it("drops an older read that succeeds last: log and cursor stay the newest read's", async () => {
    const real = api.readSessionPage;
    const fresh = await real.call(api, "pamukkale");
    let resolve!: () => void;
    const read = vi
      .spyOn(api, "readSessionPage")
      // The older read answers last, with a history that differs from the
      // newer one's in both its records and its cursor.
      .mockImplementationOnce(
        () =>
          new Promise((r) => {
            resolve = () => r({ records: fresh.records.slice(0, 1), older: "stale-cursor" });
          }),
      )
      .mockImplementation((...args) => real.apply(api, args));
    try {
      const older = state().rebuildLog("pamukkale");
      await state().rebuildLog("pamukkale");
      const log = state().logs.pamukkale;
      const history = state().histories.pamukkale;
      resolve();
      await older;
      expect(state().logs.pamukkale).toBe(log);
      expect(state().histories.pamukkale).toBe(history);
      expect(state().histories.pamukkale?.older).not.toBe("stale-cursor");
    } finally {
      read.mockRestore();
    }
    expect(state().logLoads.pamukkale).toEqual({ status: "ready" });
  });
});

describe("paged transcripts", () => {
  // caspian's fixture is seven codex rollout lines, which no earlier test
  // sends to. Pages of three cut it mid-turn: the newest opens on the tool
  // call, the prompt is a page back and the session's header two.
  const PAGE = 3;
  const RECORDS = 7;
  const realPage = api.readSessionPage;

  /** Run `fn` against pages of `PAGE` records, end to end through the mock. */
  async function withSmallPages(fn: (read: MockInstance<typeof realPage>) => Promise<void>) {
    const read = vi
      .spyOn(api, "readSessionPage")
      .mockImplementation((agentId, before) => realPage(agentId, before, PAGE));
    try {
      await fn(read);
    } finally {
      read.mockRestore();
    }
  }

  /** The log a host without pages builds: the whole history, read at once. */
  async function wholeLog(agentId: string): Promise<ChatItem[]> {
    const protocol = state().protocol;
    useStore.setState({
      protocol: protocol && {
        ...protocol,
        ops: protocol.ops.filter((o) => o !== "read_session_page"),
      },
    });
    try {
      await state().rebuildLog(agentId);
    } finally {
      useStore.setState({ protocol });
    }
    return state().logs[agentId] ?? [];
  }

  it("opens an agent on its newest page, with a cursor for the rest", async () => {
    const whole = vi.spyOn(api, "readSessionRecords");
    try {
      await withSmallPages(async (read) => {
        await state().rebuildLog("caspian");
        expect(read).toHaveBeenCalledTimes(1);
      });
      expect(whole).not.toHaveBeenCalled();
    } finally {
      whole.mockRestore();
    }
    const history = state().histories.caspian;
    expect(history?.records).toHaveLength(PAGE);
    expect(history?.older).not.toBeNull();
    const log = state().logs.caspian ?? [];
    // The newest turn's tail is there; its prompt is still on an older page.
    expect(log.some((i) => i.kind === "tool_call")).toBe(true);
    expect(log.some((i) => i.kind === "user_message")).toBe(false);
  });

  it("prepends older pages until the log is the whole history, in order", async () => {
    const whole = await wholeLog("caspian");
    expect(state().histories.caspian?.older).toBeNull();

    await withSmallPages(async () => {
      await state().rebuildLog("caspian");
      const sizes = [state().logs.caspian?.length ?? 0];
      while (state().histories.caspian?.older) {
        await state().loadOlderLog("caspian");
        sizes.push(state().logs.caspian?.length ?? 0);
      }
      // Seven records by three: the newest page and two older ones. The
      // oldest holds only the session header, which renders nothing.
      expect(sizes).toHaveLength(3);
      expect(sizes[1]).toBeGreaterThan(sizes[0]);
      expect(sizes[2]).toBeGreaterThanOrEqual(sizes[1]);
    });
    // Re-reduced across the page boundaries, the pages render exactly what the
    // whole read does: the call with its output, the prompt before both.
    expect(state().histories.caspian?.records).toHaveLength(RECORDS);
    expect(state().logs.caspian).toEqual(whole);
    expect(whole.findIndex((i) => i.kind === "user_message")).toBeLessThan(
      whole.findIndex((i) => i.kind === "tool_call"),
    );
  });

  it("offers nothing older once the history is exhausted", async () => {
    await withSmallPages(async (read) => {
      await state().rebuildLog("caspian");
      while (state().histories.caspian?.older) await state().loadOlderLog("caspian");
      read.mockClear();
      await state().loadOlderLog("caspian");
      expect(read).not.toHaveBeenCalled();
    });
    expect(state().histories.caspian?.older).toBeNull();
  });

  it("keeps a running turn's live tail when an older page is prepended", async () => {
    await withSmallPages(async () => {
      await state().rebuildLog("caspian");
      const live: ChatItem = { kind: "queued_message", text: "and the docs", turnId: "t-live" };
      useStore.setState((s) => ({
        logs: { ...s.logs, caspian: [...(s.logs.caspian ?? []), live] },
      }));
      await state().loadOlderLog("caspian");
      expect(state().logs.caspian?.at(-1)).toEqual(live);
      expect(state().logs.caspian?.filter((i) => i.kind === "queued_message")).toHaveLength(1);
    });
  });

  it("rejects a failed older page to its caller, leaving the log and lastError alone", async () => {
    await withSmallPages(async (read) => {
      await state().rebuildLog("caspian");
      const before = state().histories.caspian;
      const log = state().logs.caspian;
      useStore.setState({ lastError: null });
      read.mockRejectedValueOnce(new Error("socket closed"));
      await expect(state().loadOlderLog("caspian")).rejects.toThrow("socket closed");
      expect(state().histories.caspian).toBe(before);
      expect(state().logs.caspian).toBe(log);
      expect(state().lastError).toBeNull();
    });
  });

  it("drops an older page a rebuild overtook", async () => {
    await withSmallPages(async (read) => {
      await state().rebuildLog("caspian");
      let release = () => {};
      read.mockImplementationOnce(async (agentId, before) => {
        await new Promise<void>((r) => {
          release = r;
        });
        return realPage(agentId, before, PAGE);
      });
      const older = state().loadOlderLog("caspian");
      await state().rebuildLog("caspian");
      const rebuilt = state().logs.caspian;
      release();
      await older;
      expect(state().logs.caspian).toBe(rebuilt);
      expect(state().histories.caspian?.records).toHaveLength(PAGE);
    });
  });

  it("reads the whole history from a host without the op", async () => {
    const page = vi.spyOn(api, "readSessionPage");
    const records = vi.spyOn(api, "readSessionRecords");
    try {
      const log = await wholeLog("caspian");
      expect(page).not.toHaveBeenCalled();
      expect(records).toHaveBeenCalledWith("caspian");
      expect(state().histories.caspian).toMatchObject({ older: null });
      expect(state().histories.caspian?.records).toHaveLength(RECORDS);
      expect(log.some((i) => i.kind === "user_message")).toBe(true);
    } finally {
      page.mockRestore();
      records.mockRestore();
    }
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

  it("loadPrLive keeps the last checks when the live read has none, replaces them when it does", async () => {
    await state().loadGit("kamakura");
    const before = state().prChecks.kamakura;
    const pr = state().prStates.kamakura;
    expect(before).toBeTruthy();
    expect(pr).toBeTruthy();
    if (!before || !pr) return;
    const spy = vi.spyOn(api, "getPrLive");
    try {
      // The CI read didn't resolve this round: the state is fresh, the tint
      // is whatever it was.
      spy.mockResolvedValueOnce({ state: { ...pr, title: "renamed on GitHub" }, checks: null });
      await state().loadPrLive("kamakura");
      expect(state().prStates.kamakura?.title).toBe("renamed on GitHub");
      expect(state().prChecks.kamakura).toBe(before);
      // A real rollup replaces it.
      spy.mockResolvedValueOnce({ state: pr, checks: { ...before, passed: 13, failed: 1 } });
      await state().loadPrLive("kamakura");
      expect(state().prChecks.kamakura?.failed).toBe(1);
      // Nothing to say at all leaves both alone.
      spy.mockResolvedValueOnce(null);
      await state().loadPrLive("kamakura");
      expect(state().prChecks.kamakura?.failed).toBe(1);
      expect(state().prStates.kamakura?.title).toBe(pr.title);
    } finally {
      spy.mockRestore();
    }
  });

  it("loadPrThreads asks nothing of a host without get_pr_threads", async () => {
    expect(state().hostSupports("get_pr_threads")).toBe(false);
    const spy = vi.spyOn(api, "getPrThreads");
    try {
      await state().loadPrThreads("kamakura");
      expect(spy).not.toHaveBeenCalled();
      expect(state().prComments.kamakura).toBeUndefined();
    } finally {
      spy.mockRestore();
    }
  });

  it("loadPrThreads stores the threads once the host lists the op", async () => {
    const protocol = state().protocol;
    const spy = vi.spyOn(api, "getPrThreads").mockResolvedValue({ unresolved: [] });
    useStore.setState({
      protocol: protocol && { ...protocol, ops: [...protocol.ops, "get_pr_threads"] },
    });
    try {
      await state().loadPrThreads("kamakura");
      expect(spy).toHaveBeenCalledWith("kamakura");
      expect(state().prComments.kamakura).toEqual({ unresolved: [] });
    } finally {
      useStore.setState({ protocol });
      spy.mockRestore();
    }
  });

  it("keeps the latest verify:report per agent", () => {
    const report = {
      checks: [{ name: "test", command: "bun test", outcome: "passed", duration_ms: 1, tail: [] }],
    };
    hostEvent("verify:report", { agent_id: "arabia", report });
    expect(state().verificationReports.arabia).toEqual(report);
    hostEvent("verify:report", { agent_id: "arabia", report: { checks: [] } });
    expect(state().verificationReports.arabia).toEqual({ checks: [] });
  });

  it("writes a line of ship activity when a PR opens, read against the record it replaces", () => {
    useStore.setState((s) => ({
      prStates: { ...s.prStates, pamukkale: null },
      shipActivity: { ...s.shipActivity, pamukkale: [] },
    }));
    const opened = {
      number: 650,
      url: "https://github.com/o/r/pull/650",
      state: "open",
      title: "t",
      mergeable: "unknown",
    };
    hostEvent("pr:state_changed", { agent_id: "pamukkale", state: opened });
    expect(state().shipActivity.pamukkale?.map((e) => e.text)).toEqual(["PR #650 opened"]);
    // The same state again is not a transition.
    hostEvent("pr:state_changed", { agent_id: "pamukkale", state: opened });
    expect(state().shipActivity.pamukkale).toHaveLength(1);
    // A secondary repo's PR is not this agent's PR on the phone.
    hostEvent("pr:state_changed", {
      agent_id: "pamukkale",
      subdir: "api",
      state: { ...opened, number: 12, state: "merged" },
    });
    expect(state().prStates.pamukkale?.state).toBe("open");
    expect(state().shipActivity.pamukkale).toHaveLength(1);
  });

  it("takes the watcher's checks and writes a line when they settle", () => {
    useStore.setState((s) => ({
      prChecks: { ...s.prChecks, pamukkale: null },
      shipActivity: { ...s.shipActivity, pamukkale: [] },
    }));
    const checks = {
      merge_state: "unstable",
      rollup: "failing",
      total: 2,
      passed: 1,
      failed: 1,
      pending: 0,
      required_failing: ["unit"],
      runs: [],
    };
    hostEvent("pr:checks_changed", { agent_id: "pamukkale", subdir: null, checks });
    expect(state().prChecks.pamukkale).toEqual(checks);
    expect(state().shipActivity.pamukkale?.map((e) => e.text)).toEqual(["Checks failing: unit"]);
    // The same verdict with another failing name refreshes the list, not the log.
    hostEvent("pr:checks_changed", {
      agent_id: "pamukkale",
      subdir: null,
      checks: { ...checks, required_failing: ["lint"] },
    });
    expect(state().shipActivity.pamukkale).toHaveLength(1);
    // A secondary repo's checks are not this agent's PR on the phone.
    hostEvent("pr:checks_changed", {
      agent_id: "pamukkale",
      subdir: "api",
      checks: { ...checks, rollup: "passing" },
    });
    expect(state().prChecks.pamukkale?.rollup).toBe("failing");
  });

  it("takes the watcher's review threads and names the new one's author", () => {
    useStore.setState((s) => ({ shipActivity: { ...s.shipActivity, pamukkale: [] } }));
    const thread = (id: string, author: string) => ({
      id,
      author,
      is_bot: false,
      body: "",
      path: null,
      line: null,
      url: "",
      replies: 0,
      we_replied_last: false,
    });
    const comments = { unresolved: [thread("t1", "greptile"), thread("t2", "alex")] };
    hostEvent("pr:threads_changed", {
      agent_id: "pamukkale",
      subdir: null,
      comments,
      new_thread_ids: ["t2"],
    });
    expect(state().prComments.pamukkale).toEqual(comments);
    expect(state().shipActivity.pamukkale?.map((e) => e.text)).toEqual([
      "New review comment from alex",
    ]);
    hostEvent("pr:threads_changed", {
      agent_id: "pamukkale",
      subdir: null,
      comments,
      new_thread_ids: ["t1", "t2"],
    });
    expect(state().shipActivity.pamukkale?.[0].text).toBe("2 new review comments");
  });

  it("lands a tapped checks alert on the agent's Ship tab", () => {
    state().openFromPush({ hostId: MOCK_HOST_KEY, agentId: "pamukkale", kind: "checks_settled" });
    const top = state().nav[state().nav.length - 1];
    expect(top.screen).toBe("agent");
    expect(top.props).toMatchObject({ agentId: "pamukkale", tab: "ship" });
    // A turn ending is answered in the chat, as before.
    state().openFromPush({ hostId: MOCK_HOST_KEY, agentId: "pamukkale", kind: "turn_complete" });
    const chat = state().nav[state().nav.length - 1];
    expect(chat.props.tab).toBeUndefined();
  });

  it("caps an agent's ship activity at 20, newest first", () => {
    useStore.setState((s) => ({ shipActivity: { ...s.shipActivity, pamukkale: [] } }));
    for (let i = 0; i < 25; i += 1) {
      hostEvent("agent:git-action", { agent_id: "pamukkale", op: `op${i}` });
    }
    const lines = state().shipActivity.pamukkale ?? [];
    expect(lines).toHaveLength(20);
    expect(lines[0].text).toBe("Agent ran op24");
    expect(lines.at(-1)?.text).toBe("Agent ran op5");
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

  it("loadPrStatus tints every primary repo from one sweep, keeping what the sweep is silent on", async () => {
    await state().loadPrStatus();
    expect(state().prStates.kamakura?.number).toBe(642);
    expect(state().prChecks.kamakura?.passed).toBe(14);
    const pr = state().prStates.kamakura;
    const before = state().prChecks.kamakura;
    if (!pr || !before) throw new Error("the mock host binds a PR to kamakura");
    // An agent the next sweep does not name keeps its state: absence is
    // "nothing to say", not "no PR".
    const elsewhere = { ...pr, number: 7 };
    useStore.setState((s) => ({ prStates: { ...s.prStates, pamukkale: elsewhere } }));
    const spy = vi.spyOn(api, "getAllPrStatus").mockResolvedValue({
      // The CI read didn't resolve this round: fresh state, last tint.
      kamakura: { state: { ...pr, title: "renamed on GitHub" }, checks: null },
      // A secondary repo, which the phone has no row for yet.
      "kamakura::packages/app": { state: { ...pr, number: 9 }, checks: { ...before, failed: 3 } },
    });
    try {
      await state().loadPrStatus();
      expect(state().prStates.kamakura?.title).toBe("renamed on GitHub");
      expect(state().prChecks.kamakura).toBe(before);
      expect(state().prStates["kamakura::packages/app"]).toBeUndefined();
      expect(state().prChecks["kamakura::packages/app"]).toBeUndefined();
      expect(state().prStates.pamukkale).toBe(elsewhere);
      // A real rollup replaces the tint.
      spy.mockResolvedValue({ kamakura: { state: pr, checks: { ...before, failed: 1 } } });
      await state().loadPrStatus();
      expect(state().prChecks.kamakura?.failed).toBe(1);
    } finally {
      spy.mockRestore();
      useStore.setState((s) => {
        const { pamukkale: _, ...prStates } = s.prStates;
        return { prStates };
      });
    }
  });

  it("loadPrStatus asks nothing of a host without get_all_pr_status", async () => {
    const protocol = state().protocol;
    const spy = vi.spyOn(api, "getAllPrStatus");
    useStore.setState({
      protocol: protocol && {
        ...protocol,
        ops: protocol.ops.filter((o) => o !== "get_all_pr_status"),
      },
    });
    try {
      expect(state().hostSupports("get_all_pr_status")).toBe(false);
      await state().loadPrStatus();
      expect(spy).not.toHaveBeenCalled();
    } finally {
      useStore.setState({ protocol });
      spy.mockRestore();
    }
  });

  it("reads the checkout tree", async () => {
    await state().loadTree("arabia");
    expect(state().trees.arabia?.some((f) => f.status === "M")).toBe(true);
  });

  it("follows a new PR's checks and review threads from the host's events, polling neither", async () => {
    const live = vi.spyOn(api, "getPrLive");
    const threads = vi.spyOn(api, "getPrThreads");
    try {
      await state().publish("pamukkale", "feat: watch me", "");
      expect(state().prChecks.pamukkale?.rollup).toBe("pending");
      // The mock host plays the watcher: CI settles, then a reviewer writes.
      await vi.waitFor(() => expect(state().prChecks.pamukkale?.rollup).toBe("passing"));
      await vi.waitFor(() => expect(state().prComments.pamukkale?.unresolved).toHaveLength(1));
      const lines = state().shipActivity.pamukkale?.map((e) => e.text) ?? [];
      expect(lines).toContain("Checks passed");
      expect(lines).toContain("New review comment from greptile");
      expect(live).not.toHaveBeenCalled();
      expect(threads).not.toHaveBeenCalled();
    } finally {
      live.mockRestore();
      threads.mockRestore();
      // The fixtures are module state the mock host writes through; later
      // files see pamukkale without a PR, as before.
      delete fixturePrStates.pamukkale;
      delete fixturePrChecks.pamukkale;
      useStore.setState((s) => {
        const { pamukkale: _state, ...prStates } = s.prStates;
        const { pamukkale: _checks, ...prChecks } = s.prChecks;
        const { pamukkale: _comments, ...prComments } = s.prComments;
        return { prStates, prChecks, prComments };
      });
    }
  });
});

describe("auto-archive notice", () => {
  it("gathers what the host's idle sweep archived until Home dismisses it", () => {
    expect(state().autoArchived).toBeNull();
    hostEvent("workspace:auto-archived", { agent_ids: ["a1"], names: ["fuji"] });
    hostEvent("workspace:auto-archived", { agent_ids: ["a2", "a3"], names: ["kyoto", "nara"] });
    expect(state().autoArchived).toEqual(["fuji", "kyoto", "nara"]);
    state().dismissAutoArchived();
    expect(state().autoArchived).toBeNull();
    // A pass that archived nothing says nothing.
    hostEvent("workspace:auto-archived", { agent_ids: [], names: [] });
    expect(state().autoArchived).toBeNull();
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

  it("keeps a prompt raised while `approvals_list` was in flight", async () => {
    // The host forwards events and writes op responses from separate tasks, so
    // a publish gated a moment after the queue was read reaches the phone
    // first. Replacing the queue wholesale used to drop it, and the host then
    // waits out its timeout with nothing on screen to answer it.
    const waiting = { ...request, id: "pub-already-waiting" };
    const raised = { ...request, id: "pub-mid-flight" };
    const list = vi.spyOn(api, "listPublishApprovals").mockImplementation(async () => {
      state().receivePublishApproval(raised);
      return [waiting];
    });

    await state().loadPendingApprovals();

    expect(state().pendingPublishApprovals.map((r) => r.id)).toEqual([waiting.id, raised.id]);
    list.mockRestore();
    useStore.setState({ pendingPublishApprovals: [] });
  });

  it("lets the newest of two overlapping loads own the queue", async () => {
    // Every handshake starts a load without waiting for the last, so two are
    // out at once and can answer out of order. The older one answering last
    // used to land its stale snapshot over the newer one's.
    const answers: ((rows: (typeof request)[]) => void)[] = [];
    const list = vi
      .spyOn(api, "listPublishApprovals")
      .mockImplementation(() => new Promise((resolve) => answers.push(resolve)));

    const older = state().loadPendingApprovals();
    const newer = state().loadPendingApprovals();
    // Raised while both are out: it belongs to the newer read, the only one
    // whose snapshot will be written.
    const raised = { ...request, id: "pub-mid-flight" };
    state().receivePublishApproval(raised);

    answers[1]([{ ...request, id: "pub-from-the-newer-read" }]);
    await newer;
    answers[0]([{ ...request, id: "pub-from-the-older-read" }]);
    await older;

    expect(state().pendingPublishApprovals.map((r) => r.id)).toEqual([
      "pub-from-the-newer-read",
      raised.id,
    ]);
    list.mockRestore();
    useStore.setState({ pendingPublishApprovals: [] });
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

  /** Spawn an agent and wait until its first turn has a tool call streaming. */
  async function spawnMidTurn(name: string, prompt: string) {
    await state().spawn({
      repoPath: state().workspace?.projects[0].path ?? "",
      provider: "claude",
      model: "claude-opus-5",
      effort: "high",
      base: "main",
      prompt,
      name,
    });
    const id = state().workspace?.agents[0]?.id ?? "";
    await vi.waitFor(
      () => {
        expect(agentOf(state(), id)?.status).toBe("running");
        expect((state().logs[id] ?? []).some((i) => i.kind === "tool_call")).toBe(true);
      },
      { timeout: 10_000 },
    );
    return id;
  }

  it("replays the running turn from the host when the agent is re-opened mid-turn", async () => {
    const prompt = "Wire the dictation engine picker";
    const id = await spawnMidTurn("lofoten", prompt);
    const live = state().logs[id] ?? [];
    // A real host has ingested nothing of the running turn: no record of it
    // yet, and its user turn still unmatched. The mock persists every step, so
    // hold both back. And the phone's socket died in the background, so its
    // own log missed the whole turn.
    const turns = (await api.readUserTurns(id)).map((t) => ({
      ...t,
      native_id: null,
      ended_at: null,
      position: null,
    }));
    const read = vi.spyOn(api, "readSessionPage").mockResolvedValue({ records: [], older: null });
    const readTurns = vi.spyOn(api, "readUserTurns").mockResolvedValue(turns);
    useStore.setState((s) => ({ logs: { ...s.logs, [id]: [] } }));
    try {
      // Back to the list, then into the agent again: what openAgent runs.
      state().pop();
      await state().loadAgent(id);
    } finally {
      read.mockRestore();
      readTurns.mockRestore();
    }
    const after = state().logs[id] ?? [];
    // The prompt opens the replayed turn, then everything the stream drew.
    expect(after[0]).toMatchObject({ kind: "user_message", text: prompt });
    for (const item of live.filter((i) => i.kind !== "user_message")) {
      expect(after).toContainEqual(item);
    }

    // Once the turn ends the host's records are complete and authoritative:
    // re-opening then does rebuild from them.
    await vi.waitFor(() => expect(agentOf(state(), id)?.status).toBe("idle"), {
      timeout: 10_000,
    });
    useStore.setState((s) => ({ logs: { ...s.logs, [id]: (s.logs[id] ?? []).slice(0, 1) } }));
    await state().loadAgent(id);
    expect((state().logs[id] ?? []).some((i) => i.kind === "tool_call")).toBe(true);
  }, 30_000);

  it("keeps the live log mid-turn on a host without read_live_turn", async () => {
    const id = await spawnMidTurn("senja", "Add the relay setting to the host sheet");
    const live = state().logs[id] ?? [];
    const protocol = state().protocol;
    const { records } = await api.readSessionPage(id);
    const read = vi
      .spyOn(api, "readSessionPage")
      .mockResolvedValue({ records: records.slice(0, 1), older: null });
    const readLive = vi.spyOn(api, "readLiveTurn");
    useStore.setState({
      protocol: protocol && {
        ...protocol,
        ops: protocol.ops.filter((o) => o !== "read_live_turn"),
      },
    });
    try {
      state().pop();
      await state().loadAgent(id);
    } finally {
      useStore.setState({ protocol });
      read.mockRestore();
      readLive.mockRestore();
    }
    // Nothing to replay from, so rebuilding would have left the prompt alone:
    // the stream's log stays (and keeps growing with the stream).
    expect(readLive).not.toHaveBeenCalled();
    const after = state().logs[id] ?? [];
    expect(after.length).toBeGreaterThanOrEqual(live.length);
    for (const item of live) expect(after).toContainEqual(item);
    await vi.waitFor(() => expect(agentOf(state(), id)?.status).toBe("idle"), {
      timeout: 10_000,
    });
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
    expect(state().sending[id]).toBeFalsy();
    expect(state().logs[id]).toBeUndefined();
    send.mockRestore();
  });
});

/** The `sending` flag is set on send and discharged by the status the send
 *  produces. A backgrounded webview or a dropped socket misses that silently,
 *  so every fresh snapshot reconciles it — otherwise every surface reads
 *  "working" for an agent that finished while the phone was away. */
describe("sending flag", () => {
  it("clears against a fresh snapshot when the turn ended off-socket", async () => {
    useStore.setState((s) => ({ sending: { ...s.sending, caspian: true } }));
    await state().refreshWorkspace();
    expect(agentOf(state(), "caspian")?.status).not.toBe("running");
    expect(state().sending.caspian).toBeUndefined();
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
      expect(state().sending.caspian).toBe(true);
      // The host cannot have flipped the agent to running yet — the snapshot
      // is older than the tap, so it must not clear the flag.
      await state().refreshWorkspace();
      expect(state().sending.caspian).toBe(true);
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

  it("follows a model or effort change, which the composer reads off the record", async () => {
    await state().loadChats(projectId);
    await state().setModel("sakura", "opus");
    await vi.waitFor(() => expect(agentOf(state(), "sakura")?.model).toBe("opus"));
    await state().setEffort("sakura", "high");
    await vi.waitFor(() => expect(agentOf(state(), "sakura")?.effort).toBe("high"));
  });

  it("deletes a chat outright — the registry first, then the host", async () => {
    await state().startPlanningChat({ projectId, prompt: "A queue nobody asked for" });
    const id = state().chats[projectId]?.[0]?.id ?? "";
    const discard = vi.spyOn(api, "discardAgent");
    try {
      await state().deleteChat(id);
      expect(discard).toHaveBeenCalledWith(id);
    } finally {
      discard.mockRestore();
    }
    expect(state().chats[projectId]?.some((c) => c.id === id)).toBe(false);
    // Gone on the host too, not merely hidden: the list read no longer has it.
    await state().loadChats(projectId);
    expect(state().chats[projectId]?.some((c) => c.id === id)).toBe(false);
  });

  it("puts the list back when the host refuses the delete", async () => {
    await state().loadChats(projectId);
    const discard = vi.spyOn(api, "discardAgent").mockRejectedValue(new Error("chat is busy"));
    try {
      await expect(state().deleteChat("sakura")).rejects.toThrow("chat is busy");
    } finally {
      discard.mockRestore();
    }
    // The optimistic removal was wrong, so the host's own list is read back.
    expect(state().chats[projectId]?.map((c) => c.id)).toContain("sakura");
    expect(state().lastError).toContain("chat is busy");
  });

  it("refuses to archive a chat — an archived one could never be listed again", async () => {
    await state().loadChats(projectId);
    const before = state().chats;
    const archive = vi.spyOn(api, "archiveAgent");
    try {
      await expect(state().archive("sakura")).rejects.toThrow(/deleted, not archived/);
      expect(archive).not.toHaveBeenCalled();
    } finally {
      archive.mockRestore();
    }
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

/** Delegations are the host's (`delegate_git`, `delegation:changed`): the phone
 *  asks, and mirrors what the host says about it into the Ship strip and the
 *  activity list. The mock host runs the same lifecycle in miniature. */
describe("delegations", () => {
  const AGENT = "kamakura";
  const idle = () =>
    vi.waitFor(() => expect(agentOf(state(), AGENT)?.status).toBe("idle"), { timeout: 5000 });
  const lines = () => (state().shipActivity[AGENT] ?? []).map((e) => e.text);
  const sentTexts = () =>
    (state().logs[AGENT] ?? []).flatMap((i) =>
      i.kind === "user_message" || i.kind === "queued_message" ? [i.text] : [],
    );

  it("hands an idle agent's playbook to the host, which runs it and reports the outcome", async () => {
    await idle();
    useStore.setState((s) => ({ shipActivity: { ...s.shipActivity, [AGENT]: [] } }));

    await state().delegateGit(AGENT, "commit-pr", { base: "main" });
    // The strip turns at once, off the host's record.
    expect(state().delegations[AGENT]?.kind).toBe("commit-pr");

    await vi.waitFor(() => expect(state().delegations[AGENT]).toBeUndefined(), { timeout: 5000 });
    expect(sentTexts()).toContain('[app-action] commit-pr base="main"');
    expect(lines()).toContain("Asked the agent to commit & open a PR");
    expect(lines()[0]).toBe("Committed — PR is open");
  });

  it("holds a playbook asked of a running agent until its turn ends, then runs it as its own", async () => {
    await idle();
    useStore.setState((s) => ({ shipActivity: { ...s.shipActivity, [AGENT]: [] } }));
    await state().send(AGENT, "keep going");
    await vi.waitFor(() => expect(agentOf(state(), AGENT)?.status).toBe("running"), {
      timeout: 5000,
    });

    await state().delegateGit(AGENT, "fix-checks", { failing: "unit" });
    expect(state().delegations[AGENT]?.phase).toBe("queued");
    expect(sentTexts()).not.toContain('[app-action] fix-checks failing="unit"');

    await vi.waitFor(() => expect(state().delegations[AGENT]).toBeUndefined(), { timeout: 5000 });
    const sent = sentTexts();
    // Its own turn, after the one it waited behind — not folded into it.
    expect(sent.indexOf('[app-action] fix-checks failing="unit"')).toBeGreaterThan(
      sent.lastIndexOf("keep going"),
    );
    expect(lines()).toEqual([
      "Agent finished — checks are re-running",
      "Asked the agent to fix the failing checks once its turn ends",
    ]);
  });

  it("sends the trigger as a plain message to a host without delegate_git", async () => {
    await idle();
    const protocol = state().protocol;
    useStore.setState({
      protocol: protocol && { ...protocol, ops: protocol.ops.filter((o) => o !== "delegate_git") },
    });
    const delegate = vi.spyOn(api, "delegateGit");
    const send = vi.spyOn(api, "sendUserMessage");
    try {
      await state().delegateGit(AGENT, "commit");
      expect(delegate).not.toHaveBeenCalled();
      expect(send).toHaveBeenCalledWith(AGENT, expect.any(String), "[app-action] commit", []);
      expect(lines()[0]).toBe("Asked the agent to commit");
    } finally {
      useStore.setState({ protocol });
      delegate.mockRestore();
      send.mockRestore();
      await idle();
    }
  });

  it("re-reads the host's table, primary repos only, and asks no host without the op", async () => {
    const rows = [
      { agent_id: AGENT, subdir: null, kind: "push", phase: "running", started_at: 5 },
      { agent_id: AGENT, subdir: "api", kind: "commit", phase: "queued", started_at: 6 },
    ] as const;
    const spy = vi.spyOn(api, "getDelegations").mockResolvedValue([...rows]);
    const protocol = state().protocol;
    try {
      await state().loadDelegations();
      expect(state().delegations).toEqual({ [AGENT]: rows[0] });

      spy.mockClear();
      useStore.setState({
        protocol: protocol && {
          ...protocol,
          ops: protocol.ops.filter((o) => o !== "get_delegations"),
        },
      });
      await state().loadDelegations();
      expect(spy).not.toHaveBeenCalled();
    } finally {
      useStore.setState({ protocol, delegations: {} });
      spy.mockRestore();
    }
  });
});

/** Autopilot is the host's (docs/remote-protocol.md, "Autopilot"): the phone
 *  mirrors its state and log, re-read on every handshake, and flips only an
 *  agent's pause. The mock host has kamakura's PR mid-cycle. */
describe("autopilot", () => {
  const AGENT = "kamakura";
  const lines = () => (state().shipActivity[AGENT] ?? []).map((e) => e.text);
  const entry = (over: Partial<AutopilotLogEntry>): AutopilotLogEntry => ({
    id: "ap-live",
    agent_id: AGENT,
    subdir: null,
    at: Date.now(),
    outcome: "dispatch",
    rung: "fix-checks",
    attempt: 1,
    ...over,
  });

  it("re-reads the host's state and log on every handshake", async () => {
    useStore.setState({ autopilot: {}, shipActivity: {} });
    await state().reconnect();
    await vi.waitFor(() => expect(state().autopilot[AGENT]?.cycle).not.toBeNull());
    expect(state().autopilot[AGENT]).toMatchObject({
      enrolled: true,
      paused: false,
      project_enabled: true,
      cycle: { rung: "fix-checks", attempt: 2, phase: "awaiting-evidence" },
    });
    // Every live agent has a row, on by default.
    expect(state().autopilot.arabia).toMatchObject({ enrolled: true, cycle: null });
    await vi.waitFor(() =>
      expect(lines()).toEqual([
        "Autopilot started on the failing checks, try 2",
        "Autopilot's try 1 on the failing checks didn't work",
        "Autopilot started on the failing checks",
      ]),
    );
  });

  it("folds the live events, a seeded row and its event being one line", () => {
    // The row the handshake already read, arriving late as its event.
    hostEvent("autopilot:event", entry({ id: "ap-kamakura-3", attempt: 2 }));
    expect(lines().filter((l) => l.startsWith("Autopilot started"))).toHaveLength(2);

    hostEvent(
      "autopilot:event",
      entry({ id: "ap-live", outcome: "give-up", attempt: 3, reason: "budget-spent" }),
    );
    expect(lines()[0]).toBe("Autopilot gave up on the failing checks after 3 tries");

    const row = state().autopilot[AGENT];
    hostEvent("autopilot:state", { ...row, cycle: null });
    expect(state().autopilot[AGENT]?.cycle).toBeNull();
    // A secondary checkout is a row of its own, beside the primary's.
    hostEvent("autopilot:state", { ...row, subdir: "api" });
    expect(state().autopilot[`${AGENT}::api`]?.cycle?.rung).toBe("fix-checks");
    expect(state().autopilot[AGENT]?.cycle).toBeNull();
  });

  it("takes the host's switches whole, though no row changed", () => {
    const rows = state().autopilot;
    useStore.setState({ autopilotSwitches: { disabled_projects: [], paused_agents: [AGENT] } });

    // A project with no agents switched off on the Mac: no `autopilot:state`.
    hostEvent("autopilot:switches", { disabled_projects: ["empty"], paused_agents: [] });

    expect(state().autopilotSwitches).toEqual({ disabled_projects: ["empty"], paused_agents: [] });
    expect(state().autopilot).toBe(rows);
  });

  it("pauses and resumes an agent through the host, taking its answer as the state", async () => {
    const spy = vi.spyOn(api, "setAutopilot");
    try {
      await state().reconnect();
      await vi.waitFor(() => expect(state().autopilot[AGENT]?.cycle).not.toBeNull());

      await state().setAgentAutopilot(AGENT, false);
      expect(spy).toHaveBeenCalledWith({ agentId: AGENT }, false);
      // The pause drops the cycle on the host, and the answer says so.
      expect(state().autopilot[AGENT]).toMatchObject({
        paused: true,
        enrolled: false,
        cycle: null,
      });
      expect(state().autopilot.arabia?.paused).toBe(false);
      expect(state().autopilotSwitches?.paused_agents).toEqual([AGENT]);

      await state().setAgentAutopilot(AGENT, true);
      expect(spy).toHaveBeenLastCalledWith({ agentId: AGENT }, true);
      expect(state().autopilot[AGENT]).toMatchObject({
        paused: false,
        enrolled: true,
        cycle: null,
      });
    } finally {
      spy.mockRestore();
    }
  });

  it("asks nothing of a host, or a pairing, without the ops", async () => {
    const protocol = state().protocol;
    const read = vi.spyOn(api, "getAutopilotState");
    const log = vi.spyOn(api, "getAutopilotLog");
    const flip = vi.spyOn(api, "setAutopilot");
    useStore.setState({
      protocol: protocol && {
        ...protocol,
        ops: protocol.ops.filter((o) => !o.startsWith("autopilot_")),
      },
    });
    try {
      await state().loadAutopilot();
      await state().setAgentAutopilot(AGENT, false);
      expect(read).not.toHaveBeenCalled();
      expect(log).not.toHaveBeenCalled();
      expect(flip).not.toHaveBeenCalled();
    } finally {
      useStore.setState({ protocol });
      read.mockRestore();
      log.mockRestore();
      flip.mockRestore();
    }
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
