// The Home header's host switcher: the chip names the live host, and its
// dropdown lists every paired one, a tap from switching, with adding a host
// and the Host sheet (where the active one is unpaired) below. The store's actions are stubbed; what
// they do is multiHost.test's business.

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { HostSwitcher } from "../src/screens/Home/HostSwitcher";
import { useStore } from "../src/store";

const HOSTS = [
  { hostKey: "mini", name: "Mac mini", host: "10.0.0.5", port: 47285, pairedAt: "2026-01-01" },
  { hostKey: "studio", name: "Studio Mac", host: "10.0.0.7", port: 47285, pairedAt: "2026-02-01" },
];

const actions = {
  switchHost: vi.fn(async (_hostKey: string) => {}),
  addHost: vi.fn(),
  openSheet: vi.fn(),
};

const button = (root: HTMLElement, text: string) =>
  [...root.querySelectorAll("button")].find((b) => b.textContent?.includes(text));
const menu = (root: HTMLElement) => root.querySelector(".host-menu");

let host: HTMLElement;
let unmount: () => void;

beforeEach(async () => {
  vi.clearAllMocks();
  useStore.setState({ ...actions, hosts: HOSTS, hostKey: "mini", connection: "connected" });
  host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(createElement(HostSwitcher)));
  unmount = () => act(() => root.unmount());
});

afterEach(() => unmount());

const open = () => act(async () => host.querySelector<HTMLButtonElement>(".host")?.click());

test("the chip names the live host and opens a list of every paired one", async () => {
  const chip = host.querySelector(".host");
  expect(chip?.textContent).toContain("Mac mini");
  expect(chip?.querySelector(".dot.running")).not.toBeNull();
  expect(menu(host)).toBeNull();

  await open();
  const rows = [...host.querySelectorAll('[role="menuitemradio"]')];
  expect(rows.map((r) => r.querySelector(".lbl")?.textContent)).toEqual(["Mac mini", "Studio Mac"]);
  // The live host carries the connection dot and state; the other is unknown.
  expect(rows[0].getAttribute("aria-checked")).toBe("true");
  expect(rows[0].textContent).toContain("Connected");
  expect(rows[1].getAttribute("aria-checked")).toBe("false");
  expect(rows[1].querySelector(".dot.idle")).not.toBeNull();
});

test("tapping another host switches to it and closes the list", async () => {
  await open();
  await act(async () => button(host, "Studio Mac")?.click());
  expect(actions.switchHost).toHaveBeenCalledWith("studio");
  expect(menu(host)).toBeNull();
});

test("switching still works while the live host cannot be reached", async () => {
  await act(async () => useStore.setState({ connection: "error" }));
  expect(host.querySelector(".host .dot.error")).not.toBeNull();
  await open();
  await act(async () => button(host, "Studio Mac")?.click());
  expect(actions.switchHost).toHaveBeenCalledWith("studio");
});

test("adds a host, or opens the Host sheet, from below the list", async () => {
  await open();
  await act(async () => button(host, "Add a host…")?.click());
  expect(actions.addHost).toHaveBeenCalledOnce();
  expect(menu(host)).toBeNull();

  await open();
  await act(async () => button(host, "Host details")?.click());
  expect(actions.openSheet).toHaveBeenCalledWith("host");
});

test("tapping outside closes the list without doing anything", async () => {
  await open();
  await act(async () => host.querySelector<HTMLButtonElement>(".host-menu-bg")?.click());
  expect(menu(host)).toBeNull();
  expect(actions.switchHost).not.toHaveBeenCalled();
});
