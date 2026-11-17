// The Home header's host chip. With one host paired it names that host and
// whether it is reachable, and its dropdown holds the host (checked), adding
// another and the Host sheet. With several it counts them — green only while
// every one is connected — and its dropdown lists each with its own
// connection, a tap from that host's Host sheet. The store's actions are
// stubbed; what they do is multiHost.test's business.

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { HostSwitcher } from "../src/screens/Home/HostSwitcher";
import { appStore, hostStoreOf } from "../src/store";
import type { SavedHost } from "../src/store/persist";
import { MINI, onHost, PAIRED_MINI, PAIRED_STUDIO, STUDIO } from "./support";

const actions = {
  switchHost: vi.fn(async (_hostKey: string) => {}),
  addHost: vi.fn(),
  openSheet: vi.fn(),
};

const button = (root: HTMLElement, text: string) =>
  [...root.querySelectorAll("button")].find((b) => b.textContent?.includes(text));
const menu = (root: HTMLElement) => root.querySelector(".host-menu");
/** The dropdown's host rows: the ones carrying a connection dot. */
const hostRows = (root: HTMLElement) =>
  [...root.querySelectorAll(".host-menu .row")].filter((r) => r.querySelector(".dot"));
const chip = (root: HTMLElement) => root.querySelector(".host");

let host: HTMLElement;
let unmount: () => void;

async function render(hosts: SavedHost[]) {
  appStore.setState({ ...actions, hosts, hostKey: MINI });
  host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(onHost(MINI, createElement(HostSwitcher))));
  unmount = () => act(() => root.unmount());
}

const open = () => act(async () => host.querySelector<HTMLButtonElement>(".host")?.click());

beforeEach(() => {
  vi.clearAllMocks();
  hostStoreOf(MINI).setState({ connection: "connected" });
  hostStoreOf(STUDIO).setState({ connection: "connecting" });
});

afterEach(() => unmount());

describe("with one host", () => {
  beforeEach(() => render([PAIRED_MINI]));

  test("the chip names the host and whether it is reachable", async () => {
    expect(chip(host)?.textContent).toContain("Mac mini");
    expect(chip(host)?.querySelector(".dot.running")).not.toBeNull();
    // The chip opens the host list; it is amber for anything short of
    // connected, and never red.
    await act(async () => hostStoreOf(MINI).setState({ connection: "error" }));
    expect(chip(host)?.querySelector(".dot.waiting")).not.toBeNull();
    expect(menu(host)).toBeNull();
  });

  test("the list holds the host, checked, then adding another and its Host sheet", async () => {
    await open();
    const rows = hostRows(host);
    expect(rows.map((r) => r.querySelector(".lbl")?.textContent)).toEqual(["Mac mini"]);
    expect(rows[0].getAttribute("role")).toBe("menuitemradio");
    expect(rows[0].getAttribute("aria-checked")).toBe("true");
    expect(rows[0].textContent).toContain("Connected");

    await act(async () => button(host, "Host details")?.click());
    // The sheet of the host on screen, by name: it stays that host's sheet.
    expect(actions.openSheet).toHaveBeenCalledWith("host", { hostKey: MINI });
    expect(menu(host)).toBeNull();

    await open();
    await act(async () => button(host, "Add a host…")?.click());
    expect(actions.addHost).toHaveBeenCalledOnce();
    expect(menu(host)).toBeNull();
  });
});

describe("with several hosts", () => {
  beforeEach(() => render([PAIRED_MINI, PAIRED_STUDIO]));

  test("the chip counts them, green only while every one is connected", async () => {
    expect(chip(host)?.textContent).toContain("2 hosts");
    expect(chip(host)?.textContent).not.toContain("Mac mini");
    expect(chip(host)?.querySelector(".dot.waiting")).not.toBeNull();
    await act(async () => hostStoreOf(STUDIO).setState({ connection: "connected" }));
    expect(chip(host)?.querySelector(".dot.running")).not.toBeNull();
    await act(async () => hostStoreOf(MINI).setState({ connection: "error" }));
    expect(chip(host)?.querySelector(".dot.waiting")).not.toBeNull();
  });

  test("the list holds every host with its own connection, none of them checked", async () => {
    await open();
    const rows = hostRows(host);
    expect(rows.map((r) => r.querySelector(".lbl")?.textContent)).toEqual([
      "Mac mini",
      "Studio Mac",
    ]);
    expect(rows.map((r) => r.querySelector(".sub")?.textContent)).toEqual([
      "Connected",
      "Connecting…",
    ]);
    expect(rows[0].querySelector(".dot.running")).not.toBeNull();
    expect(rows[1].querySelector(".dot.waiting")).not.toBeNull();
    for (const row of rows) {
      expect(row.getAttribute("role")).toBe("menuitem");
      expect(row.hasAttribute("aria-checked")).toBe(false);
    }
    // Each host's details are its own row's.
    expect(button(host, "Host details")).toBeUndefined();
    expect(button(host, "Add a host…")).toBeDefined();
  });

  test("a host shows its own connection as it changes", async () => {
    await open();
    const studio = () => hostRows(host)[1];
    await act(async () => hostStoreOf(STUDIO).setState({ connection: "connected" }));
    expect(studio().querySelector(".dot.running")).not.toBeNull();
    expect(studio().classList).not.toContain("inactive");
    // Out of reach, with the client retrying on its own: greyed, not red.
    await act(async () => hostStoreOf(STUDIO).setState({ connection: "error", retrying: true }));
    expect(studio().querySelector(".dot.stopped")).not.toBeNull();
    expect(studio().classList).toContain("inactive");
    // Stopped on something only the user can clear: red.
    await act(async () => hostStoreOf(STUDIO).setState({ connection: "error", retrying: false }));
    expect(studio().querySelector(".dot.error")).not.toBeNull();
    expect(studio().classList).not.toContain("inactive");
    expect(hostRows(host)[0].querySelector(".dot.running")).not.toBeNull();
  });

  test("tapping a host opens its Host sheet rather than switching, down or not", async () => {
    await open();
    await act(async () => button(host, "Studio Mac")?.click());
    expect(actions.openSheet).toHaveBeenCalledWith("host", { hostKey: STUDIO });
    expect(menu(host)).toBeNull();

    await act(async () => hostStoreOf(MINI).setState({ connection: "error" }));
    await open();
    await act(async () => button(host, "Mac mini")?.click());
    expect(actions.openSheet).toHaveBeenLastCalledWith("host", { hostKey: MINI });
    expect(actions.switchHost).not.toHaveBeenCalled();
  });

  test("adds a host from below the list", async () => {
    await open();
    await act(async () => button(host, "Add a host…")?.click());
    expect(actions.addHost).toHaveBeenCalledOnce();
    expect(menu(host)).toBeNull();
  });

  test("tapping outside closes the list without doing anything", async () => {
    await open();
    await act(async () => host.querySelector<HTMLButtonElement>(".host-menu-bg")?.click());
    expect(menu(host)).toBeNull();
    expect(actions.openSheet).not.toHaveBeenCalled();
    expect(actions.switchHost).not.toHaveBeenCalled();
  });
});
