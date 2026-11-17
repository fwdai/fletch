// Every paired host live at once, over the real stores and the real protocol
// client: two hosts the mock answers as themselves (`registerMockHost`), each
// with its own name and workspace, and an agent id the two share. As in
// multiHost.test, the URL leaves mock mode before `init`, so the store reads
// both hosts from the settings file and dials them as it would real ones.
//
// One file, in order, because `init` runs once per module registry.

import type { AgentRecord, Workspace } from "@desktop/api/types/agent";
import type { SessionRecord } from "@desktop/api/types/session";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { App } from "../src/App";
import { linkOf } from "../src/host";
import type { HelloResult } from "../src/remote";
import { registerMockHost } from "../src/remote/mock";
import * as fx from "../src/remote/mock/fixtures";
import {
  agentOf,
  appStore,
  type HostState,
  hostStoreOf,
  RESUME_PROBE_TIMEOUT_MS,
} from "../src/store";
import { isAgentOpen } from "../src/store/hostNav";
import { loadSettings } from "../src/store/persist";
import {
  foreground,
  hostEvent,
  hostOnScreen,
  MINI,
  PAIRED_MINI,
  PAIRED_STUDIO,
  pairedOnDisk,
  STUDIO,
  settle,
} from "./support";

/** The studio's own history: what tells its `arabia` from the mini's. */
const said = (seq: number, type: "user" | "assistant", text: string): SessionRecord => ({
  session_id: "studio-session",
  seq,
  provider: "claude",
  source: "transcript",
  native_id: `studio-${seq}`,
  agent_version: null,
  body: {
    type,
    message: {
      role: type,
      ...(type === "assistant" ? { model: "claude-fable-5-1" } : {}),
      content: [{ type: "text", text }],
    },
  },
});

const arabia = fx.workspace.agents.find((a) => a.id === "arabia") as AgentRecord;

/** The studio runs the same project, with an `arabia` of its own and an agent
 *  the mini does not have. */
const STUDIO_WORKSPACE: Workspace = {
  repos: [fx.FLETCH_REPO],
  projects: [fx.workspace.projects[0]],
  agents: [
    { ...arabia, status: "idle", title: "Studio previews", task: "Render the studio's previews" },
    { ...arabia, id: "lofoten", name: "lofoten", status: "idle", title: "Only on the studio" },
  ],
};

const MINI_SAYS = "Put dictation model choice in general settings.";
const STUDIO_SAYS = "Render the studio's previews.";

const app = () => appStore.getState();
const mini = () => hostStoreOf(MINI).getState();
const studio = () => hostStoreOf(STUDIO).getState();
const ids = (s: HostState) => s.workspace?.agents.map((a) => a.id) ?? [];
const logText = (s: HostState, agentId: string) => JSON.stringify(s.logs[agentId] ?? []);
const top = () => app().nav[app().nav.length - 1];
const home = () => ({
  key: Date.now(),
  screen: "home" as const,
  props: {},
  phase: "idle" as const,
});

/** What a reconnect stubbed out answers with. */
const snapshotOf = (hostKey: string): HelloResult => {
  const { client } = linkOf(hostKey);
  return {
    host: client.host ?? { name: "", appVersion: "", os: "" },
    workspace: hostStoreOf(hostKey).getState().workspace,
    protocol: client.protocol ?? undefined,
  };
};

let restoreUrl: () => void;

beforeAll(async () => {
  registerMockHost(MINI, { host: { name: "Mac mini", appVersion: "0.7.23", os: "macos" } });
  registerMockHost(STUDIO, {
    host: { name: "Studio Mac", appVersion: "0.7.23", os: "macos" },
    workspace: STUDIO_WORKSPACE,
    records: {
      arabia: [said(1, "user", STUDIO_SAYS), said(2, "assistant", "Rendering on the studio.")],
      lofoten: [],
    },
    turns: {},
  });
  restoreUrl = pairedOnDisk([PAIRED_MINI, PAIRED_STUDIO], MINI);
  await app().init();
  await vi.waitFor(
    () => {
      for (const s of [mini(), studio()]) {
        expect(s.connection).toBe("connected");
        expect(s.workspace).not.toBeNull();
      }
    },
    { timeout: 5000 },
  );
});

afterAll(() => restoreUrl());

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("every paired host, live at once", () => {
  it("connects every paired host on launch, each on its own link, the saved one on screen", () => {
    expect(app().hostKey).toBe(MINI);
    expect(hostOnScreen()).toBe(hostStoreOf(MINI));
    expect(linkOf(MINI)).not.toBe(linkOf(STUDIO));
    expect(linkOf(MINI).client.state).toBe("connected");
    expect(linkOf(STUDIO).client.state).toBe("connected");
    // Each store holds its own host.
    expect(mini().hostInfo?.name).toBe("Mac mini");
    expect(studio().hostInfo?.name).toBe("Studio Mac");
    expect(ids(mini())).toEqual(expect.arrayContaining(["arabia", "pamukkale", "kamakura"]));
    expect(ids(mini())).not.toContain("lofoten");
    expect(ids(studio())).toEqual(["arabia", "lofoten"]);
    expect(app().hosts.map((h) => h.name)).toEqual(["Mac mini", "Studio Mac"]);
  });

  it("switches to the other host at once, keeping the one left connected and its data", async () => {
    const kept = [{ kind: "user_message" as const, text: "held by the mini", turnId: "t" }];
    hostStoreOf(MINI).setState((s) => ({ logs: { ...s.logs, kamakura: kept } }));
    const clients = [linkOf(MINI).client, linkOf(STUDIO).client];
    const connects = clients.map((c) => vi.spyOn(c, "connect"));
    const drops = clients.map((c) => vi.spyOn(c, "disconnect"));

    const switching = app().switchHost(STUDIO);
    // On screen with its workspace before anything is awaited.
    expect(app().hostKey).toBe(STUDIO);
    expect(ids(hostOnScreen().getState())).toEqual(["arabia", "lofoten"]);
    await switching;

    for (const spy of [...connects, ...drops]) expect(spy).not.toHaveBeenCalled();
    expect(linkOf(MINI).client.state).toBe("connected");
    expect(mini().logs.kamakura).toBe(kept);
    expect((await loadSettings()).activeHostKey).toBe(STUDIO);

    await app().switchHost(MINI);
    expect(app().hostKey).toBe(MINI);
    expect(hostOnScreen().getState().logs.kamakura).toBe(kept);
    for (const spy of connects) expect(spy).not.toHaveBeenCalled();
  });

  it("keeps a host that is not on screen current: its events land in its own store", async () => {
    expect(app().hostKey).toBe(MINI);
    // Changed on the studio (as from its Mac): the host announces it with
    // `agent:model`, which the studio's store takes with the mini on screen.
    await linkOf(STUDIO).api.setAgentModel("arabia", "claude-haiku-4-5");
    await vi.waitFor(() => expect(agentOf(studio(), "arabia")?.model).toBe("claude-haiku-4-5"));
    // The same id on the mini is another agent, and nothing reached it.
    expect(agentOf(mini(), "arabia")?.model).toBe("claude-fable-5-1");
  });

  it("keeps the same agent id apart on the two hosts, from the history to the open screen", async () => {
    await Promise.all([mini().loadAgent("arabia"), studio().loadAgent("arabia")]);
    expect(logText(mini(), "arabia")).toContain(MINI_SAYS);
    expect(logText(mini(), "arabia")).not.toContain(STUDIO_SAYS);
    expect(logText(studio(), "arabia")).toContain(STUDIO_SAYS);
    expect(logText(studio(), "arabia")).not.toContain(MINI_SAYS);

    await studio().send("arabia", "Only for the studio");
    expect(logText(studio(), "arabia")).toContain("Only for the studio");
    expect(logText(mini(), "arabia")).not.toContain("Only for the studio");

    // Open on the studio — with the mini on screen — and open there only.
    studio().openAgent("arabia");
    expect(top().props).toEqual({ agentId: "arabia", hostKey: STUDIO });
    expect(isAgentOpen(STUDIO, "arabia")).toBe(true);
    expect(isAgentOpen(MINI, "arabia")).toBe(false);

    // A turn ending on the mini's `arabia` has no screen open to re-read for;
    // one on the studio's has.
    const miniRead = vi.spyOn(linkOf(MINI).api, "readSessionPage");
    const studioRead = vi.spyOn(linkOf(STUDIO).api, "readSessionPage");
    hostEvent("session:records-appended", { agent_id: "arabia" }, MINI);
    hostEvent("session:records-appended", { agent_id: "arabia" }, STUDIO);
    await vi.waitFor(() => expect(studioRead).toHaveBeenCalledWith("arabia"));
    await settle();
    expect(miniRead).not.toHaveBeenCalled();
    appStore.setState({ nav: [home()] });
  });

  it("opens a tapped alert from the other host on that host, dropping nothing", async () => {
    expect(app().hostKey).toBe(MINI);
    const clients = [linkOf(MINI).client, linkOf(STUDIO).client];
    const connects = clients.map((c) => vi.spyOn(c, "connect"));
    const drops = clients.map((c) => vi.spyOn(c, "disconnect"));

    app().openFromPush({ hostId: STUDIO, agentId: "lofoten", kind: "turn_complete" });
    expect(app().hostKey).toBe(STUDIO);
    expect(top().screen).toBe("agent");
    expect(top().props).toEqual({ agentId: "lofoten", hostKey: STUDIO });
    await vi.waitFor(() => expect(studio().logLoads.lofoten).toEqual({ status: "ready" }));

    for (const spy of [...connects, ...drops]) expect(spy).not.toHaveBeenCalled();
    expect(linkOf(MINI).client.state).toBe("connected");
    await app().switchHost(MINI);
  });
});

describe("returning to the foreground", () => {
  it("probes every host, and reconnects only the one that misses", async () => {
    vi.useFakeTimers();
    const miniRead = vi.spyOn(linkOf(MINI).api, "getWorkspace");
    // A socket iOS froze: the request goes out and nothing ever comes back.
    const studioRead = vi
      .spyOn(linkOf(STUDIO).api, "getWorkspace")
      .mockReturnValue(new Promise(() => {}));
    const miniReconnect = vi.spyOn(linkOf(MINI).client, "reconnect");
    const studioReconnect = vi
      .spyOn(linkOf(STUDIO).client, "reconnect")
      .mockResolvedValue(snapshotOf(STUDIO));

    foreground();
    await vi.advanceTimersByTimeAsync(RESUME_PROBE_TIMEOUT_MS - 1);
    expect(miniRead).toHaveBeenCalled();
    expect(studioRead).toHaveBeenCalled();
    expect(studioReconnect).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(2);
    expect(studioReconnect).toHaveBeenCalledTimes(1);
    expect(miniReconnect).not.toHaveBeenCalled();
  });

  it("dials a host waiting out a retry at once, and probes the one that is up", async () => {
    const { client } = linkOf(STUDIO);
    vi.spyOn(client, "state", "get").mockReturnValue("error");
    vi.spyOn(client, "retrying", "get").mockReturnValue(true);
    const studioReconnect = vi.spyOn(client, "reconnect").mockResolvedValue(snapshotOf(STUDIO));
    const studioRead = vi.spyOn(linkOf(STUDIO).api, "getWorkspace");
    const miniRead = vi.spyOn(linkOf(MINI).api, "getWorkspace");

    foreground();
    await vi.waitFor(() => expect(studioReconnect).toHaveBeenCalledTimes(1));
    await vi.waitFor(() => expect(miniRead).toHaveBeenCalled());
    expect(studioRead).not.toHaveBeenCalled();
  });
});

describe("a screen names the host it was opened for", () => {
  it("keeps acting on that host after another is put on screen", async () => {
    const node = document.createElement("div");
    document.body.append(node);
    const root = createRoot(node);
    await act(async () => root.render(createElement(App)));
    const screens = () => [...node.querySelectorAll(".scr")];

    // The mini's fletch project, opened from Home the way a tap opens it.
    const card = [...node.querySelectorAll<HTMLElement>(".pc")].find(
      (c) => c.querySelector(".nm")?.textContent === "fletch",
    );
    await act(async () => card?.click());
    expect(top().props).toEqual({ projectId: "prj-fletch", hostKey: MINI });

    // The studio put on screen under it — here without the stack reset that
    // comes with every way of doing so, so the screen is still there to read.
    await act(async () => appStore.setState({ hostKey: STUDIO }));
    const project = screens().at(-1);
    // Its rows are every host's, each naming its own…
    expect(project?.textContent).toContain("pamukkale · Mac mini");

    // …and opening on it, while what the screen starts is the mini's still.
    const push = vi.fn();
    const openSheet = vi.fn();
    appStore.setState({ push, openSheet });
    const row = [...(project?.querySelectorAll<HTMLElement>(".ag") ?? [])].find((r) =>
      r.textContent?.includes("pamukkale"),
    );
    await act(async () => row?.click());
    expect(push).toHaveBeenCalledWith("agent", { agentId: "pamukkale", hostKey: MINI });
    await act(async () => project?.querySelector<HTMLElement>('[aria-label="New agent"]')?.click());
    expect(openSheet).toHaveBeenCalledWith("newAgent", { projectId: "prj-fletch", hostKey: MINI });

    act(() => root.unmount());
    node.remove();
    appStore.setState({
      push: appStore.getInitialState().push,
      openSheet: appStore.getInitialState().openSheet,
      hostKey: MINI,
      nav: [home()],
    });
  });
});

describe("the working-tree poll", () => {
  it("reads every connected host, whichever is on screen", async () => {
    const stats = [MINI, STUDIO].map((key) => vi.spyOn(linkOf(key).api, "getAllShortstats"));
    const node = document.createElement("div");
    const root = createRoot(node);
    await act(async () => root.render(createElement(App)));
    expect(app().hostKey).toBe(MINI);
    for (const read of stats) expect(read).toHaveBeenCalledOnce();

    // Polled all along: put on screen, the studio is not read again for it.
    await act(async () => app().switchHost(STUDIO));
    expect(stats[1]).toHaveBeenCalledOnce();

    act(() => root.unmount());
    await app().switchHost(MINI);
  });
});

describe("forgetting a host", () => {
  it("forgets one that is not on screen, and leaves the one on screen alone", async () => {
    expect(app().hostKey).toBe(MINI);
    const studioLink = linkOf(STUDIO);
    // A screen still open on the studio: it goes with the host.
    appStore.setState({
      nav: [
        home(),
        { key: 2, screen: "agent", props: { agentId: "lofoten", hostKey: STUDIO }, phase: "idle" },
      ],
    });

    await app().unpair(STUDIO);

    expect(app().hostKey).toBe(MINI);
    expect(app().hosts.map((h) => h.hostKey)).toEqual([MINI]);
    expect(app().nav.map((n) => n.screen)).toEqual(["home"]);
    const saved = await loadSettings();
    expect(saved.hosts?.map((h) => h.hostKey)).toEqual([MINI]);
    expect(saved.activeHostKey).toBe(MINI);
    // Its link is stopped and gone; the mini's never noticed.
    expect(studioLink.client.state).toBe("disconnected");
    expect(linkOf(STUDIO)).not.toBe(studioLink);
    expect(linkOf(MINI).client.state).toBe("connected");
    expect(hostOnScreen()).toBe(hostStoreOf(MINI));
  });
});
