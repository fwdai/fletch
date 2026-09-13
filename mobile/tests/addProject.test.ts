// The add-project flows end to end over the mock host: the real protocol
// client, the real store actions, the real persist layer. The jsdom URL in
// vite.config.ts puts this file in mock mode (see `mockEnabled`).

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { childPath, parentPath } from "../src/lib/paths";
import { MOCK_HOST_KEY } from "../src/remote/mock";
import { HOME } from "../src/remote/mock/fixtures";
import { AddProjectSheet } from "../src/sheets/AddProjectSheet";
import { CloneForm } from "../src/sheets/AddProjectSheet/CloneForm";
import { OpenFolderForm } from "../src/sheets/AddProjectSheet/OpenFolderForm";
import { api, useStore } from "../src/store";
import { loadDestParent } from "../src/store/persist";

const state = () => useStore.getState();

/** Where the picker would land after browsing to `~/Code` — the absolute base
 *  the host reports, not the string it was asked for. */
const codeDir = async () => (await state().listDir("~/Code")).base;

beforeAll(async () => {
  await state().init();
  await vi.waitFor(() => expect(state().connection).toBe("connected"), { timeout: 5000 });
});

describe("path arithmetic", () => {
  it("joins a listing base with an entry name without doubling the separator", () => {
    // A bare `~` comes back expanded *with* a trailing slash, so this is the
    // real shape, not a defensive edge case.
    expect(childPath(`${HOME}/`, "Code")).toBe("/Users/alex/Code");
    expect(childPath(HOME, "Code")).toBe("/Users/alex/Code");
    expect(childPath("/", "Users")).toBe("/Users");
  });

  it("walks back up, and stops at the root", () => {
    expect(parentPath("/Users/alex/Code")).toBe("/Users/alex");
    expect(parentPath("/Users/alex/")).toBe("/Users");
    expect(parentPath("/Users")).toBe("/");
    expect(parentPath("/")).toBeNull();
  });
});

describe("browsing the host", () => {
  it("reports the absolute base for a tilde path, and directories to walk", async () => {
    const listing = await state().listDir("~");
    expect(listing.base).toBe(`${HOME}/`);
    expect(listing.entries).toContainEqual({ name: "Code", is_dir: true });
    // Files come back too; the picker is what filters them out.
    expect(listing.entries.some((e) => !e.is_dir)).toBe(true);
  });

  it("records a failed listing in lastError and rethrows it", async () => {
    await expect(state().listDir("/nope")).rejects.toThrow("no such directory");
    expect(state().lastError).toContain("no such directory");
  });
});

describe("open an existing folder", () => {
  it("sends the picked absolute path and takes the workspace the host returns", async () => {
    const spy = vi.spyOn(api, "addWorkspaceRepo");
    const picked = childPath(await codeDir(), "playground");
    await state().addWorkspaceRepo(picked);
    expect(spy).toHaveBeenCalledWith("/Users/alex/Code/playground");
    // No `workspace:changed` follows these ops, so the returned workspace is
    // the only thing that puts the project on the home screen.
    expect(state().workspace?.projects.map((p) => p.path)).toContain(picked);
    spy.mockRestore();
  });

  it("pinning a folder that is already a project changes nothing", async () => {
    const picked = childPath(await codeDir(), "playground");
    const before = state().workspace;
    await state().addWorkspaceRepo(picked);
    expect(state().workspace?.projects).toEqual(before?.projects);
  });

  it("records the host's refusal in lastError and rethrows it", async () => {
    // The host refuses a folder nested inside an existing repository rather
    // than running `git init` in it.
    const nested = childPath(childPath(await codeDir(), "playground"), "src");
    await expect(state().addWorkspaceRepo(nested)).rejects.toThrow("inside the git repository");
    expect(state().lastError).toContain("inside the git repository");
  });
});

describe("clone from GitHub", () => {
  it("knows whether gh is signed in, and lists the user's repos", async () => {
    expect(await state().ghStatus()).toMatchObject({
      authenticated: true,
      login: "alexchaplinsky",
    });
    expect((await state().ghRepoList()).map((r) => r.name_with_owner)).toContain("fwdai/fletch");
  });

  it("clones into the chosen parent and remembers it for this host", async () => {
    expect(state().lastDestParent).toBeNull();
    const parent = await codeDir();
    await state().cloneRepo("https://github.com/fwdai/relay.git", parent);
    expect(state().workspace?.projects.map((p) => p.path)).toContain(`${parent}/relay`);
    expect(state().lastDestParent).toBe(parent);
    // What the next session would hydrate as the clone form's default.
    expect(await loadDestParent(MOCK_HOST_KEY)).toBe(parent);
  });

  it("reuses the remembered parent, and surfaces the collision it causes", async () => {
    const spy = vi.spyOn(api, "cloneRepo");
    const remembered = state().lastDestParent ?? "";
    await expect(state().cloneRepo("fwdai/relay", remembered)).rejects.toThrow(
      `a folder already exists at ${remembered}/relay`,
    );
    expect(spy).toHaveBeenCalledWith("fwdai/relay", remembered);
    expect(state().lastError).toContain("a folder already exists at");
    spy.mockRestore();
  });

  it("closes only the Add Project sheet when a slow clone finishes", async () => {
    let finish: (w: NonNullable<ReturnType<typeof state>["workspace"]>) => void = () => {};
    const spy = vi
      .spyOn(api, "cloneRepo")
      .mockReturnValue(new Promise((resolve) => (finish = resolve)));
    state().openSheet("addProject");
    const cloning = state().cloneRepo("fwdai/other", await codeDir());

    // The user gave up waiting and opened something else.
    state().closeSheet();
    state().openSheet("host");
    finish(state().workspace as NonNullable<ReturnType<typeof state>["workspace"]>);
    await cloning;

    expect(state().sheet).toMatchObject({ name: "host", open: true });
    spy.mockRestore();
  });

  it("closeSheet still closes when wired straight to a click handler", () => {
    state().openSheet("host");
    // React hands an onClick/onClose callback its event; the unconditional
    // closer must ignore it rather than read it as a sheet name.
    (state().closeSheet as (e: unknown) => void)({ type: "click" });
    expect(state().sheet).toMatchObject({ name: "host", open: false });
  });
});

// First-render markup only — enough to catch a form that cannot render and to
// pin the two things the layout has to say before anything is picked.
describe("the sheet", () => {
  const noop = async () => {};

  it("offers both flows, and tells the user about git init", () => {
    const html = renderToStaticMarkup(
      createElement(AddProjectSheet, { open: true, onClose: () => {} }),
    );
    expect(html).toContain("Open folder");
    expect(html).toContain("Clone");
    expect(html).toContain("Use this folder");
    expect(html).toContain("git init");
  });

  it("asks for a destination, and will not clone without one", () => {
    // A render-only harness reads zustand's *initial* state, so this is the
    // nothing-remembered case; the remembered one is asserted on the store.
    const html = renderToStaticMarkup(
      createElement(CloneForm, { busy: false, error: null, setError: () => {}, run: noop }),
    );
    expect(html).toContain("Choose a folder");
    expect(html).toContain('class="btn primary block ap-use" disabled=""');
  });

  it("shows a failed op's own text, in place", () => {
    const html = renderToStaticMarkup(
      createElement(OpenFolderForm, {
        busy: false,
        error: "a folder already exists at /Users/alex/Code/relay",
        setError: () => {},
        run: noop,
      }),
    );
    expect(html).toContain("a folder already exists at /Users/alex/Code/relay");
  });
});
