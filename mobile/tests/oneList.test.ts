// Home as one list across hosts, over the real stores, the real protocol
// client and the real App: two hosts the mock answers as themselves
// (`registerMockHost`), paired on disk as in liveHosts.test. Both have `fletch` — the
// mini owns it, the studio runs a copy — and both have `relay`, which is the
// studio's own though the mini comes first. `atlas` is the mini's alone, and
// each host has a `pamukkale` of its own, working and holding a prompt.
//
// One file, in order, because `init` runs once per module registry; it ends
// with the studio forgotten, to see the phone with one host again.

import type { AgentRecord, ProjectRef, Workspace } from "@desktop/api/types/agent";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { App } from "../src/App";
import { linkOf } from "../src/host";
import { ignore } from "../src/lib/ignore";
import { type MockHostSpec, registerMockHost } from "../src/remote/mock";
import * as fx from "../src/remote/mock/fixtures";
import { agentOf, appStore, hostStoreOf } from "../src/store";
import { openSheetOn } from "../src/store/hostNav";
import { homeItem } from "../src/store/nav";
import { MINI, PAIRED_MINI, PAIRED_STUDIO, pairedOnDisk, STUDIO, settle } from "./support";

const arabia = fx.workspace.agents.find((a) => a.id === "arabia") as AgentRecord;
const pamukkale = fx.workspace.agents.find((a) => a.id === "pamukkale") as AgentRecord;
const fletch = fx.workspace.projects[0];

const STUDIO_FLETCH = "/Users/studio/fletch";
const STUDIO_RELAY = "/Users/studio/relay";
const MINI_RELAY = "/Users/alex/code/relay";

const relay = (path: string, home?: string): ProjectRef => ({
  path,
  name: "relay",
  project_id: "prj-relay",
  label: null,
  ...(home ? { home: { device: home, name: home } } : {}),
});

/** An idle agent in `projectId`, checked out at `path`. */
const idle = (id: string, projectId: string, path: string, title: string): AgentRecord => ({
  ...arabia,
  id,
  name: id,
  project_id: projectId,
  status: "idle",
  title,
  repos: [{ ...arabia.repos[0], repo_path: path }],
});

const MINI_WORKSPACE: Workspace = {
  ...fx.workspace,
  projects: [...fx.workspace.projects, relay(MINI_RELAY, STUDIO)],
  agents: [...fx.workspace.agents, idle("kyoto", "prj-relay", MINI_RELAY, "Relay on the mini")],
};

const STUDIO_SPEC: MockHostSpec = {
  host: { name: "Studio Mac", appVersion: "0.7.23", os: "macos" },
  workspace: {
    repos: [STUDIO_FLETCH, STUDIO_RELAY],
    projects: [
      { ...fletch, path: STUDIO_FLETCH, home: { device: MINI, name: "Mac mini" } },
      relay(STUDIO_RELAY),
    ],
    agents: [
      {
        ...pamukkale,
        title: "Studio previews",
        repos: [{ ...pamukkale.repos[0], repo_path: STUDIO_FLETCH }],
      },
      idle("lofoten", "prj-fletch", STUDIO_FLETCH, "Only on the studio"),
      idle("zermatt", "prj-relay", STUDIO_RELAY, "Relay on the studio"),
    ],
  },
};

const app = () => appStore.getState();
const mini = () => hostStoreOf(MINI).getState();
const studio = () => hostStoreOf(STUDIO).getState();
const top = () => app().nav[app().nav.length - 1];

let node: HTMLElement;
let unmount: () => void;
let restoreUrl: () => void;

/** The App, on Home. */
async function mount() {
  appStore.setState({ nav: [homeItem()], sheet: null });
  node = document.createElement("div");
  document.body.append(node);
  const root = createRoot(node);
  await act(async () => root.render(createElement(App)));
  unmount = () => {
    act(() => root.unmount());
    node.remove();
  };
}

const all = (selector: string) => [...node.querySelectorAll<HTMLElement>(selector)];
const card = (name: string) => all(".pc").find((c) => c.querySelector(".nm")?.textContent === name);
/** An agent row's second line: its codename, and its host when named. */
const sub = (row: Element) => row.querySelector(".ex")?.textContent;
const button = (text: string) => all("button").find((b) => b.textContent?.includes(text));
const click = (el: HTMLElement | undefined) => act(async () => el?.click());

beforeAll(async () => {
  registerMockHost(MINI, {
    host: { name: "Mac mini", appVersion: "0.7.23", os: "macos" },
    workspace: MINI_WORKSPACE,
  });
  registerMockHost(STUDIO, STUDIO_SPEC);
  restoreUrl = pairedOnDisk([PAIRED_MINI, PAIRED_STUDIO], MINI);
  await app().init();
  // Settled: both hosts up, each `pamukkale` holding the prompt a fresh mock
  // host raises, and the turn it plays on the mini's `arabia` over.
  await vi.waitFor(
    () => {
      for (const s of [mini(), studio()]) {
        expect(s.connection).toBe("connected");
        expect(Object.keys(s.pendingToolUse.pamukkale ?? {})).toHaveLength(1);
      }
      expect(agentOf(mini(), "arabia")?.status).toBe("idle");
    },
    { timeout: 5000 },
  );
});

afterAll(() => restoreUrl());

afterEach(() => {
  unmount?.();
  vi.restoreAllMocks();
  appStore.setState({ push: appStore.getInitialState().push, nav: [homeItem()], sheet: null });
});

describe("Home, with two hosts paired", () => {
  it("lists each project once, a shared one with the agents of both hosts", async () => {
    await mount();
    expect(all(".pc").map((c) => c.querySelector(".nm")?.textContent)).toEqual(
      expect.arrayContaining(["fletch", "atlas", "relay"]),
    );
    expect(all(".pc")).toHaveLength(3);

    // The mini's arabia, pamukkale and kamakura with the studio's pamukkale
    // and lofoten: one working on each host, three more resting.
    const shared = card("fletch");
    expect(shared?.querySelector(".cnt b")?.textContent).toBe("2 running");
    expect(shared?.querySelector(".cnt")?.textContent).toContain("5");
    expect([...(shared?.querySelectorAll(".ag") ?? [])].map(sub)).toEqual([
      "pamukkale · Mac mini",
      "pamukkale · Studio Mac",
    ]);
    expect(shared?.querySelector(".pc-more")?.textContent).toBe("3 more agents");

    // A project only the mini has is listed all the same.
    expect(card("atlas")?.querySelector(".ag .ex")?.textContent).toContain("caspian · Mac mini");
    // …and the header counts every host's work.
    expect(node.querySelector(".sect")?.textContent).toContain("2 running");
  });

  it("keeps an agent id two hosts share as two rows, each its own host's, opened there", async () => {
    await mount();
    const rows = () => [...(card("fletch")?.querySelectorAll<HTMLElement>(".ag") ?? [])];
    const [onMini, onStudio] = rows();
    expect(onStudio.textContent).toContain("Studio previews");
    expect(onMini.textContent).not.toContain("Studio previews");
    expect(rows().map((r) => r.textContent?.includes("needs you"))).toEqual([true, true]);
    // Answered on the studio: its row stops asking, the mini's does not.
    await act(async () => hostStoreOf(STUDIO).setState({ pendingToolUse: {} }));
    expect(rows().map((r) => r.textContent?.includes("needs you"))).toEqual([true, false]);

    const push = vi.fn();
    appStore.setState({ push });
    await click(rows()[0]);
    await click(rows()[1]);
    expect(push.mock.calls).toEqual([
      ["agent", { agentId: "pamukkale", hostKey: MINI }],
      ["agent", { agentId: "pamukkale", hostKey: STUDIO }],
    ]);
    // The rows open their agents, not the project card under them.
    expect(push.mock.calls.some(([screen]) => screen === "project")).toBe(false);
  });

  it("opens a project on the host that speaks for it: its home, else the first", async () => {
    await mount();
    await click(card("fletch"));
    // Both have it and neither copy names another home than the mini.
    expect(top().props).toEqual({ projectId: "prj-fletch", hostKey: MINI });

    appStore.setState({ nav: [homeItem()] });
    const studioChats = vi.spyOn(linkOf(STUDIO).api, "listProjectChats");
    const miniChats = vi.spyOn(linkOf(MINI).api, "listProjectChats");
    await click(card("relay"));
    // The studio's own project, though the mini is first in saved order.
    expect(top().props).toEqual({ projectId: "prj-relay", hostKey: STUDIO });
    await act(settle);
    const screen = all(".scr").at(-1);
    // Both hosts' agents in its tabs, each naming its host…
    expect([...(screen?.querySelectorAll(".ag") ?? [])].map(sub)).toEqual([
      "kyoto · Mac mini",
      "zermatt · Studio Mac",
    ]);
    // …and its planning chats read from the studio alone.
    expect(studioChats).toHaveBeenCalledWith("prj-relay", expect.anything());
    expect(miniChats).not.toHaveBeenCalled();
  });

  it("names a host that is down over the list, and keeps its rows", async () => {
    await mount();
    expect(node.querySelector(".conn")).toBeNull();
    // The studio stops answering.
    registerMockHost(STUDIO, { ...STUDIO_SPEC, unreachable: true });
    await act(async () => {
      await studio().reconnect().catch(ignore);
    });
    await vi.waitFor(() => expect(studio().connection).toBe("error"));
    await act(settle);

    expect(node.querySelector(".conn")?.textContent).toContain("Studio Mac is offline");
    // Its rows are still there, as they were last seen — and the list with them.
    expect([...(card("fletch")?.querySelectorAll(".ag") ?? [])].map(sub)).toContain(
      "pamukkale · Studio Mac",
    );
    expect(node.querySelector(".blank")).toBeNull();
    // The chip says not every host is up.
    expect(node.querySelector(".host .dot.waiting")).not.toBeNull();

    // Back again, at a tap.
    registerMockHost(STUDIO, STUDIO_SPEC);
    await click(button("Retry"));
    await vi.waitFor(() => expect(studio().connection).toBe("connected"), { timeout: 5000 });
    await act(settle);
    expect(node.querySelector(".conn")).toBeNull();
    expect(node.querySelector(".host .dot.running")).not.toBeNull();
  });

  it("starts an agent on the host of the project picked in the New agent sheet", async () => {
    await mount();
    const spawns = [MINI, STUDIO].map((key) => vi.spyOn(linkOf(key).api, "spawnAgent"));
    const studioNames = vi.spyOn(linkOf(STUDIO).api, "allocateDraftName");
    await click(button("New agent"));
    // Opened on the first project, on the host that speaks for it.
    expect(app().sheet).toMatchObject({
      name: "newAgent",
      open: true,
      props: { hostKey: MINI, projectId: "prj-fletch" },
    });

    await click(node.querySelector<HTMLElement>(".na-mark .pj") ?? undefined);
    const relayRow = all(".row").find((r) => r.querySelector(".lbl")?.textContent === "relay");
    await click(relayRow);
    await act(settle);
    // The sheet has moved to the studio: its name is drawn there, and it says so.
    expect(app().sheet?.props).toEqual({ hostKey: STUDIO, projectId: "prj-relay" });
    expect(studioNames).toHaveBeenCalled();
    expect(node.querySelector(".na-sub")?.textContent).toContain("created on Studio Mac");

    const field = node.querySelector("textarea");
    if (!field) throw new Error("no prompt field");
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(
        field,
        "Wire the relay's retries",
      );
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
    // The agent opens where it was started; its screen is not this test's.
    const push = vi.fn();
    appStore.setState({ push });
    await click(all("button").find((b) => b.textContent?.startsWith("Start ")));
    await vi.waitFor(() => expect(push).toHaveBeenCalled(), { timeout: 5000 });

    expect(spawns[1]).toHaveBeenCalledOnce();
    expect(spawns[1].mock.calls[0][0]).toBe(STUDIO_RELAY);
    expect(spawns[0]).not.toHaveBeenCalled();
    expect(push).toHaveBeenCalledWith("agent", expect.objectContaining({ hostKey: STUDIO }));
    // …and the sheet, the studio's now, is closed by it.
    expect(app().sheet?.open).toBe(false);
  });

  it("asks which host Add project is for, on screen first, and browses that one", async () => {
    await mount();
    await click(node.querySelector<HTMLElement>(".sect-add") ?? undefined);
    const chips = () => all(".host-pick .chip");
    expect(chips().map((c) => c.textContent)).toEqual(["Mac mini", "Studio Mac"]);
    expect(node.querySelector(".host-pick .chip.on")?.textContent).toBe("Mac mini");

    const studioList = vi.spyOn(linkOf(STUDIO).api, "listDir");
    await click(chips()[1]);
    // The sheet is the studio's now, so what lands there closes it.
    expect(app().sheet).toMatchObject({ name: "addProject", props: { hostKey: STUDIO } });
    expect(node.querySelector(".host-pick .chip.on")?.textContent).toBe("Studio Mac");
    await vi.waitFor(() => expect(studioList).toHaveBeenCalled());
  });

  it("reaches each host's providers from its Host sheet, not the header", async () => {
    await mount();
    expect(node.querySelector(".host-cog")).toBeNull();
    await act(async () => openSheetOn(STUDIO, "host"));
    await click(button("Providers and accounts"));
    expect(top().screen).toBe("providers");
    expect(top().props.hostKey).toBe(STUDIO);
  });
});

describe("Home, with one host left", () => {
  it("names no host on a row, the chip names the host, and Add project asks nothing", async () => {
    await app().unpair(STUDIO);
    expect(app().hosts.map((h) => h.hostKey)).toEqual([MINI]);
    await mount();

    expect(all(".pc").map((c) => c.querySelector(".nm")?.textContent)).toEqual(
      expect.arrayContaining(["fletch", "atlas", "relay"]),
    );
    expect([...(card("fletch")?.querySelectorAll(".ag") ?? [])].map(sub)).toEqual(["pamukkale"]);
    expect(node.querySelector(".host")?.textContent).toContain("Mac mini");
    expect(node.querySelector(".host-cog")).not.toBeNull();

    await click(node.querySelector<HTMLElement>(".sect-add") ?? undefined);
    expect(node.querySelector(".host-pick")).toBeNull();
  });
});
