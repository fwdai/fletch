// "Runs on" in the New agent sheet, over the real stores, the real protocol
// client and the real App, with the two hosts of oneList.test: the mini owns
// `fletch` and `atlas`, the studio runs a copy of `fletch` and owns `relay`.
// So `fletch` can run on either as it is, while `atlas` reaches the studio only
// as a copy the phone asks for: its definition from the mini, then
// `ensure_project` on the studio.
//
// One file, in order, because `init` runs once per module registry; the studio
// takes its copy of `atlas` in the last case with two hosts, and the file ends
// with the studio forgotten.

import type { AgentRecord, ProjectRef, Workspace } from "@desktop/api/types/agent";
import { GATES } from "@desktop/store/gates";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { App } from "../src/App";
import { linkOf } from "../src/host";
import { projectById } from "../src/lib/projects";
import { type MockHostSpec, registerMockHost } from "../src/remote/mock";
import * as fx from "../src/remote/mock/fixtures";
import { appStore, hostStoreOf } from "../src/store";
import { homeItem } from "../src/store/nav";
import {
  deferred,
  MINI,
  PAIRED_MINI,
  PAIRED_STUDIO,
  pairedOnDisk,
  STUDIO,
  settle,
} from "./support";

const fletch = fx.workspace.projects[0];
const STUDIO_FLETCH = "/Users/studio/fletch";
const STUDIO_RELAY = "/Users/studio/relay";
/** Where the studio keeps its copy of `atlas` (the mock's `ensure_project`). */
const STUDIO_ATLAS = `${fx.HOME}/.fletch/projects/prj-atlas/repos/atlas/atlas`;

const relay = (path: string, home?: string): ProjectRef => ({
  path,
  name: "relay",
  project_id: "prj-relay",
  label: null,
  ...(home ? { home: { device: home, name: home } } : {}),
});

const MINI_SPEC: MockHostSpec = {
  host: { name: "Mac mini", appVersion: "0.7.23", os: "macos" },
  workspace: { ...fx.workspace, projects: [...fx.workspace.projects, relay("/m/relay", STUDIO)] },
};

const STUDIO_SPEC: MockHostSpec = {
  host: { name: "Studio Mac", appVersion: "0.7.23", os: "macos" },
  workspace: {
    repos: [STUDIO_FLETCH, STUDIO_RELAY],
    projects: [
      { ...fletch, path: STUDIO_FLETCH, home: { device: MINI, name: "Mac mini" } },
      relay(STUDIO_RELAY),
    ],
    agents: [] as AgentRecord[],
  } satisfies Workspace,
};

const app = () => appStore.getState();
const studio = () => hostStoreOf(STUDIO).getState();

let node: HTMLElement;
let unmount: () => void;
let restoreUrl: () => void;

/** The App, on Home. */
async function mount() {
  appStore.setState({ nav: [homeItem()], sheet: null, lastError: null });
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
const button = (text: string) => all("button").find((b) => b.textContent?.includes(text));
const click = (el: HTMLElement | undefined) => act(async () => el?.click());
const chips = () => all(".host-pick .chip");
const chip = (name: string) => chips().find((c) => c.textContent === name);
const picked = () => node.querySelector(".host-pick .chip.on")?.textContent;
const notes = () => all(".host-note").map((n) => n.textContent);
const start = () => all("button").find((b) => b.textContent?.startsWith("Start "));
const field = () => {
  const el = node.querySelector("textarea");
  if (!el) throw new Error("no prompt field");
  return el;
};

/** The New agent sheet, opened from Home on `project`. */
async function openNewAgent(project: string) {
  await click(button("New agent"));
  await click(node.querySelector<HTMLElement>(".na-mark .pj") ?? undefined);
  await click(all(".row").find((r) => r.querySelector(".lbl")?.textContent === project));
  await act(settle);
}

async function type(text: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(
      field(),
      text,
    );
    field().dispatchEvent(new Event("input", { bubbles: true }));
  });
}

/** The studio dialled again as `spec` answers. */
async function redialStudio(spec: MockHostSpec) {
  registerMockHost(STUDIO, spec);
  await act(async () => {
    await studio().reconnect();
  });
  await vi.waitFor(() => expect(studio().connection).toBe("connected"), { timeout: 5000 });
  await act(settle);
}

beforeAll(async () => {
  registerMockHost(MINI, MINI_SPEC);
  registerMockHost(STUDIO, STUDIO_SPEC);
  restoreUrl = pairedOnDisk([PAIRED_MINI, PAIRED_STUDIO], MINI);
  await app().init();
  await vi.waitFor(
    () => {
      for (const key of [MINI, STUDIO]) {
        expect(hostStoreOf(key).getState().connection).toBe("connected");
      }
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

describe("Runs on, with two hosts paired", () => {
  it("shows the host the agent will run on, picked, beside the other", async () => {
    await mount();
    await click(button("New agent"));
    expect(app().sheet?.props).toEqual({ hostKey: MINI, projectId: "prj-fletch" });
    expect(node.querySelector(".host-pick")?.getAttribute("aria-label")).toBe("Runs on");
    expect(chips().map((c) => c.textContent)).toEqual(["Mac mini", "Studio Mac"]);
    expect(picked()).toBe("Mac mini");
    // Both have `fletch`: nothing is greyed, and nothing is said under the row.
    expect(chips().every((c) => !c.hasAttribute("disabled"))).toBe(true);
    expect(notes()).toEqual([]);
  });

  it("moves to a host that has the project, keeping the prompt, and starts the agent there", async () => {
    await mount();
    const copies = [MINI, STUDIO].flatMap((key) => [
      vi.spyOn(linkOf(key).api, "projectDefinition"),
      vi.spyOn(linkOf(key).api, "ensureProject"),
    ]);
    const spawns = [MINI, STUDIO].map((key) => vi.spyOn(linkOf(key).api, "spawnAgent"));
    await click(button("New agent"));
    await type("Fix the flaky retry test");

    await click(chip("Studio Mac"));
    await act(settle);
    expect(app().sheet?.props).toEqual({ hostKey: STUDIO, projectId: "prj-fletch" });
    expect(picked()).toBe("Studio Mac");
    expect(node.querySelector(".na-sub")?.textContent).toContain("created on Studio Mac");
    expect(field().value).toBe("Fix the flaky retry test");
    // Its own copy runs it: nothing is carried across.
    for (const call of copies) expect(call).not.toHaveBeenCalled();

    const push = vi.fn();
    appStore.setState({ push });
    await click(start());
    await vi.waitFor(() => expect(push).toHaveBeenCalled(), { timeout: 5000 });
    expect(spawns[0]).not.toHaveBeenCalled();
    expect(spawns[1]).toHaveBeenCalledOnce();
    expect(spawns[1].mock.calls[0][0]).toBe(STUDIO_FLETCH);
    expect(push).toHaveBeenCalledWith("agent", expect.objectContaining({ hostKey: STUDIO }));
  });

  it("greys a host that can't take the project, saying why", async () => {
    const current = fx.protocol;
    const without = (op: string) => ({ ...current, ops: current.ops.filter((o) => o !== op) });
    try {
      await mount();
      await openNewAgent("atlas");
      expect(app().sheet?.props).toEqual({ hostKey: MINI, projectId: "prj-atlas" });
      // The studio has no `atlas` but can take a copy of it.
      expect(chip("Studio Mac")?.hasAttribute("disabled")).toBe(false);

      // Too old to take a copy.
      await act(async () => hostStoreOf(STUDIO).setState({ protocol: without("ensure_project") }));
      expect(chip("Studio Mac")?.hasAttribute("disabled")).toBe(true);
      expect(notes()).toEqual([`Studio Mac · ${GATES.projectCopy.reason}`]);

      // …or with nothing to take it from: the mini, which has `atlas`, can't
      // describe it.
      await act(async () => {
        hostStoreOf(STUDIO).setState({ protocol: current });
        hostStoreOf(MINI).setState({ protocol: without("project_definition") });
      });
      expect(chip("Studio Mac")?.hasAttribute("disabled")).toBe(true);
      expect(notes()).toEqual([
        "Studio Mac · Mac mini is too old to share this project. Update it.",
      ]);
      // The host the sheet is on is never greyed.
      expect(chip("Mac mini")?.hasAttribute("disabled")).toBe(false);
    } finally {
      await act(async () => {
        hostStoreOf(STUDIO).setState({ protocol: current });
        hostStoreOf(MINI).setState({ protocol: current });
      });
    }
  });

  it("stays where it was when the copy fails, prompt and all, and says why", async () => {
    await redialStudio({
      ...STUDIO_SPEC,
      failEnsure: "Couldn't clone atlas: authentication failed",
    });
    try {
      await mount();
      await openNewAgent("atlas");
      await type("Map the atlas tiles");
      await click(chip("Studio Mac"));
      await act(settle);

      expect(app().sheet?.props).toEqual({ hostKey: MINI, projectId: "prj-atlas" });
      expect(picked()).toBe("Mac mini");
      expect(node.querySelector(".na-err")?.textContent).toContain(
        "Couldn't clone atlas: authentication failed",
      );
      expect(field().value).toBe("Map the atlas tiles");
      expect(notes()).toEqual([]);
      expect(start()?.hasAttribute("disabled")).toBe(false);
      expect(projectById(studio().workspace, "prj-atlas")).toBeUndefined();
    } finally {
      await redialStudio(STUDIO_SPEC);
    }
  });

  it("copies the project to a host without it, then moves there and starts the agent there", async () => {
    await mount();
    const studioApi = linkOf(STUDIO).api;
    const ensure = studioApi.ensureProject;
    const held = deferred<void>();
    const ensures = vi.spyOn(studioApi, "ensureProject").mockImplementation(async (definition) => {
      await held.promise;
      return ensure(definition);
    });
    const described = vi.spyOn(linkOf(MINI).api, "projectDefinition");
    const spawns = [MINI, STUDIO].map((key) => vi.spyOn(linkOf(key).api, "spawnAgent"));
    await openNewAgent("atlas");
    await type("Map the atlas tiles");

    await click(chip("Studio Mac"));
    // While the studio clones, the sheet says so and holds Start; it is still
    // the mini's, and a second pick is ignored.
    await vi.waitFor(() => expect(ensures).toHaveBeenCalledOnce());
    expect(notes()).toEqual(["Preparing the project on Studio Mac…"]);
    expect(start()?.hasAttribute("disabled")).toBe(true);
    expect(app().sheet?.props).toEqual({ hostKey: MINI, projectId: "prj-atlas" });
    await click(chip("Studio Mac"));
    expect(ensures).toHaveBeenCalledOnce();

    await act(async () => {
      held.resolve();
      await settle();
    });
    // Described by the mini, its home, and taken by the studio as a copy.
    expect(described).toHaveBeenCalledWith("prj-atlas");
    expect(ensures.mock.calls[0][0]).toMatchObject({
      project_id: "prj-atlas",
      home: { device: MINI, name: "Mac mini" },
    });
    expect(projectById(studio().workspace, "prj-atlas")?.repos[0]).toMatchObject({
      path: STUDIO_ATLAS,
      home: { device: MINI },
    });
    // The sheet is the studio's now, with the prompt as it was.
    expect(app().sheet?.props).toEqual({ hostKey: STUDIO, projectId: "prj-atlas" });
    expect(picked()).toBe("Studio Mac");
    expect(notes()).toEqual([]);
    expect(field().value).toBe("Map the atlas tiles");

    const push = vi.fn();
    appStore.setState({ push });
    await click(start());
    await vi.waitFor(() => expect(push).toHaveBeenCalled(), { timeout: 5000 });
    expect(spawns[0]).not.toHaveBeenCalled();
    expect(spawns[1]).toHaveBeenCalledOnce();
    expect(spawns[1].mock.calls[0][0]).toBe(STUDIO_ATLAS);

    // Home still lists `atlas` once, with both hosts' share of it.
    await act(async () => app().closeSheet());
    const cards = all(".pc").filter((c) => c.querySelector(".nm")?.textContent === "atlas");
    expect(cards).toHaveLength(1);
  });
});

describe("Runs on, with one host left", () => {
  it("is not offered", async () => {
    await app().unpair(STUDIO);
    await mount();
    await click(button("New agent"));
    expect(app().sheet?.open).toBe(true);
    expect(node.querySelector(".na-title")).not.toBeNull();
    expect(node.querySelector(".host-pick")).toBeNull();
  });
});
