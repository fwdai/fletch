// The Pair screen's "Macs nearby" list: picking a Mac is all the addressing
// pairing needs — its `.local` name, its port and the key it claims — and the
// typed address stays one tap away for networks that block Bonjour.

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import { type HostTarget, NO_CONFIRMER_ERROR } from "../src/remote";

// 32 bytes whose first eight are 01 23 45 67 89 ab cd ef: the vector the host's
// `discovery::tests` labels, so this pins the same name from the client side.
const KEY = "ASNFZ4mrze__AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

vi.mock("../src/remote/nearby", () => ({
  browseNearby: vi.fn(async () => [{ name: "Studio Mac", hostKey: KEY, port: 47999 }]),
}));

const { PairScreen } = await import("../src/screens/Pair");
const { useStore } = await import("../src/store");

/** What React reads from a controlled input: the native setter, then `input`. */
function type(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  setter?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

const button = (host: HTMLElement, text: string) =>
  [...host.querySelectorAll("button")].find((b) => b.textContent?.includes(text)) as
    | HTMLButtonElement
    | undefined;

let unmount: (() => void) | undefined;
afterEach(() => unmount?.());

async function mount() {
  const connect = vi.fn(async (_target: HostTarget) => {});
  useStore.setState({ connect, pairStep: null, pairTarget: null, connectionError: null });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(createElement(PairScreen)));
  unmount = () => act(() => root.unmount());
  await vi.waitFor(() => expect(button(host, "Studio Mac")).toBeDefined());
  return { host, connect };
}

test("pairs with a picked Mac by asking it, at its .local name, pinning the key it announced", async () => {
  const { host, connect } = await mount();
  // No address or code field until one is asked for.
  expect(host.querySelector("#pair-host")).toBeNull();
  expect(host.querySelector("#pair-token")).toBeNull();

  await act(async () => button(host, "Studio Mac")?.click());
  expect(host.querySelector("h1")?.textContent).toBe("Pair with Studio Mac");
  await act(async () => button(host, "Pair")?.click());

  expect(connect).toHaveBeenCalledWith({
    host: "fletch-0123456789abcdef.local",
    port: 47999,
    hostKey: KEY,
    relay: undefined,
    name: "Studio Mac",
    confirm: true,
  });
});

test("the one-time code is a tap away, and pairs with the code instead", async () => {
  const { host, connect } = await mount();
  await act(async () => button(host, "Studio Mac")?.click());
  await act(async () => button(host, "Have a pairing code")?.click());
  // A code mode with no code typed has nothing to pair with.
  expect(button(host, "Pair")?.disabled).toBe(true);
  await act(async () => type(host.querySelector("#pair-token") as HTMLInputElement, "k7pq2m9x"));
  await act(async () => button(host, "Pair")?.click());

  expect(connect).toHaveBeenCalledWith(
    expect.objectContaining({ host: "fletch-0123456789abcdef.local", pairingToken: "K7PQ2M9X" }),
  );
  expect(connect.mock.calls[0][0]).not.toHaveProperty("confirm");
});

test("a host that cannot confirm sends the phone to the code", async () => {
  const { host } = await mount();
  await act(async () => useStore.setState({ connectionError: NO_CONFIRMER_ERROR }));
  expect(host.querySelector("#pair-token")).not.toBeNull();
});

test("a typed address drops the picked Mac and the key it claimed", async () => {
  const { host, connect } = await mount();
  await act(async () => button(host, "Studio Mac")?.click());
  await act(async () => button(host, "Can't find your Mac")?.click());
  await act(async () =>
    type(host.querySelector("#pair-host") as HTMLInputElement, "10.0.0.9:47285"),
  );
  await act(async () => button(host, "Pair")?.click());

  expect(connect).toHaveBeenCalledWith(
    expect.objectContaining({ host: "10.0.0.9", port: 47285, hostKey: undefined, confirm: true }),
  );
});
