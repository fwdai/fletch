// A pairing adds a host; it never replaces one. The phone keeps a list
// (store/persist), so a link for another Mac — a QR code, a tapped URL — is
// simply another entry, with no question to answer first. What is left to get
// right is the screen: over a paired host the Pair screen comes up as an
// addition, with a way back to that host.
//
// Its own file because the first case is about a link that launched the app,
// which is decided by an `init` that runs once per module registry. A link
// that arrives while the app is running is in multiHost.test.

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, it, vi } from "vitest";
import type { HostTarget } from "../src/remote";
import { PairScreen } from "../src/screens/Pair";
import { useStore } from "../src/store";

const state = () => useStore.getState();

const PAIRED = "aG9zdC1vbmU";
const LINK: HostTarget = {
  host: "192.168.1.24",
  port: 47285,
  hostKey: "YW5vdGhlcg",
  pairingToken: "K7PQ2M9X",
  name: "Studio Mac",
};

describe("a link that launched the app over a paired host", () => {
  it("pairs with the link instead of dialling the saved host, as an addition", async () => {
    localStorage.setItem(
      "fletch-mobile-settings",
      JSON.stringify({
        hosts: [
          {
            hostKey: PAIRED,
            name: "Alex's Mac mini",
            host: "10.0.0.5",
            port: 47285,
            pairedAt: "2026-01-01T00:00:00Z",
          },
        ],
        activeHostKey: PAIRED,
      }),
    );
    const targets: HostTarget[] = [];
    useStore.setState({
      connect: async (target: HostTarget) => {
        targets.push(target);
      },
    });
    const confirm = vi.spyOn(window, "confirm");
    // Out of the suite's `?mock=1`, so `init` reads the saved host rather than
    // keeping to the mock one.
    const url = window.location.href;
    window.history.replaceState({}, "", "/");

    state().pairFromLink(LINK);
    expect(targets, "held until init has read what is on disk").toEqual([]);
    await state().init();

    // The link, once — dialling the saved host too would supersede it and burn
    // its single-use code.
    expect(targets).toEqual([LINK]);
    expect(confirm).not.toHaveBeenCalled();
    // The saved host stays paired, and the Pair screen shows the link as an
    // addition over it.
    expect(state().hostKey).toBe(PAIRED);
    expect(state().hosts.map((h) => h.hostKey)).toEqual([PAIRED]);
    expect(state().addingHost).toBe(true);
    window.history.replaceState({}, "", url);
    confirm.mockRestore();
  });
});

describe("the Pair screen adding a host", () => {
  it("offers Cancel, which goes back to the paired host", async () => {
    const cancelAddHost = vi.fn(async () => {});
    useStore.setState({
      cancelAddHost,
      addingHost: true,
      pairStep: null,
      pairTarget: null,
      connectionError: null,
    });
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    await act(async () => root.render(createElement(PairScreen)));

    expect(host.querySelector("h1")?.textContent).toBe("Add another Mac");
    const cancel = [...host.querySelectorAll("button")].find((b) => b.textContent === "Cancel");
    await act(async () => cancel?.click());
    expect(cancelAddHost).toHaveBeenCalledOnce();

    // A first pairing has nowhere to go back to.
    await act(async () => useStore.setState({ addingHost: false }));
    expect(host.querySelector("h1")?.textContent).toBe("Pair with your Mac");
    expect([...host.querySelectorAll("button")].some((b) => b.textContent === "Cancel")).toBe(
      false,
    );
    act(() => root.unmount());
  });
});
