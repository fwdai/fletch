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
import { linkOf } from "../src/host";
import type { HostTarget } from "../src/remote";
import { registerMockHost } from "../src/remote/mock";
import { PairScreen } from "../src/screens/Pair";
import { appStore } from "../src/store";
import { MINI as PAIRED, PAIRED_MINI, pairedOnDisk } from "./support";

const app = () => appStore.getState();

const LINK: HostTarget = {
  host: "192.168.1.24",
  port: 47285,
  hostKey: "YW5vdGhlcg",
  pairingToken: "K7PQ2M9X",
  name: "Studio Mac",
};

describe("a link that launched the app over a paired host", () => {
  it("pairs with the link as an addition, the saved host dialled on a link of its own", async () => {
    // Out of the suite's `?mock=1`, so `init` reads the saved host rather than
    // keeping to the mock one.
    const restoreUrl = pairedOnDisk([PAIRED_MINI], PAIRED);
    const targets: HostTarget[] = [];
    appStore.setState({
      connect: async (target: HostTarget) => {
        targets.push(target);
      },
    });
    const confirm = vi.spyOn(window, "confirm");

    app().pairFromLink(LINK);
    expect(targets, "held until init has read what is on disk").toEqual([]);
    await app().init();

    // The link, once: a second pairing would supersede it and burn its
    // single-use code.
    expect(targets).toEqual([LINK]);
    expect(confirm).not.toHaveBeenCalled();
    // The saved host is another host, on another link: it is dialled as on any
    // launch, and the pairing never touches it.
    await vi.waitFor(() => expect(linkOf(PAIRED).client.state).toBe("connected"));
    expect(linkOf(PAIRED).client.target?.hostKey).toBe(PAIRED);
    // The saved host stays paired, and the Pair screen shows the link as an
    // addition over it.
    expect(app().hostKey).toBe(PAIRED);
    expect(app().hosts.map((h) => h.hostKey)).toEqual([PAIRED]);
    expect(app().addingHost).toBe(true);
    restoreUrl();
    confirm.mockRestore();
  });
});

describe("the Pair screen adding a host", () => {
  it("offers Cancel, which goes back to the paired host", async () => {
    const cancelAddHost = vi.fn(async () => {});
    appStore.setState({
      cancelAddHost,
      addingHost: true,
      pairStep: null,
      pairTarget: null,
      pairError: null,
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
    await act(async () => appStore.setState({ addingHost: false }));
    expect(host.querySelector("h1")?.textContent).toBe("Pair with your Mac");
    expect([...host.querySelectorAll("button")].some((b) => b.textContent === "Cancel")).toBe(
      false,
    );
    act(() => root.unmount());
  });

  it("says why a pairing failed until the screen is left, which calls the pairing off", async () => {
    const ASLEEP = "YXNsZWVw";
    registerMockHost(ASLEEP, { unreachable: true });
    const { connect, cancelAddHost } = appStore.getInitialState();
    appStore.setState({ connect, cancelAddHost });
    app().addHost();

    const pairing = app().connect({ ...LINK, hostKey: ASLEEP });
    await expect(pairing).rejects.toThrow("No answer from your Mac");
    expect(app().pairError).toContain("No answer from your Mac");
    expect(app().pairStep).toBeNull();
    // The paired host was never touched.
    expect(linkOf(PAIRED).client.state).toBe("connected");

    app().cancelAddHost();
    expect(app().pairError).toBeNull();
    expect(app().addingHost).toBe(false);
    expect(linkOf(ASLEEP).client.state).toBe("disconnected");
  });

  it("stops a pairing that another replaces, and leaves the screen to the newer one", async () => {
    const ASLEEP = "c2xlZXBpbmc";
    const AWAKE = "YXdha2U";
    registerMockHost(ASLEEP, { unreachable: true });
    registerMockHost(AWAKE, {});
    const { connect } = appStore.getInitialState();
    appStore.setState({ connect });
    app().addHost();

    const first = app().connect({ ...LINK, hostKey: ASLEEP });
    const second = app().connect({ ...LINK, hostKey: AWAKE });
    await expect(first).rejects.toThrow();
    // The first one's ending is not the Pair screen's business any more.
    expect(app().pairError).toBeNull();
    expect(app().pairStep).not.toBeNull();
    // And its client is not left retrying behind the second.
    expect(linkOf(ASLEEP).client.state).toBe("disconnected");

    await second;
    expect(app().hostKey).toBe(AWAKE);
    expect(app().pairStep).toBeNull();
  });
});
