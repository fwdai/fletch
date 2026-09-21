// Pairing with a second host is not an addition: the phone keeps one host
// (store/persist), so the link silently drops the one it has — its push
// registration, its chats, everything on screen. A link is a QR code or a
// tapped URL, which is exactly the kind of thing that arrives by accident, so
// the swap is a question rather than a side effect.
//
// Its own file because it drives `pairFromLink` against a store that is already
// paired, which the queue test next door deliberately is not.

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { HostTarget } from "../src/remote";
import { useStore } from "../src/store";

const state = () => useStore.getState();

const LINK = (hostKey: string): HostTarget => ({
  host: "192.168.1.24",
  port: 47285,
  hostKey,
  pairingToken: "K7PQ2M9X",
  name: "Alex's Mac",
});

const PAIRED = "aG9zdC1vbmU";

describe("a pairing link for a host other than the one on file", () => {
  let connect: (target: HostTarget) => Promise<void>;

  beforeEach(() => {
    connect = vi.fn(async (_target: HostTarget) => {});
    useStore.setState({
      connect,
      ready: true,
      pairStep: null,
      pairTarget: null,
      hostKey: PAIRED,
      hostInfo: { name: "Alex's Mac mini" } as never,
    });
  });

  it("asks first, naming the host that would be dropped", () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);

    state().pairFromLink(LINK("YW5vdGhlcg"));

    expect(confirm).toHaveBeenCalledOnce();
    expect(confirm.mock.calls[0][0]).toContain("Alex's Mac mini");
    expect(connect).toHaveBeenCalledOnce();
    confirm.mockRestore();
  });

  it("drops the link when the answer is no", () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);

    state().pairFromLink(LINK("YW5vdGhlcg"));

    expect(connect).not.toHaveBeenCalled();
    // Nothing is left on the Pair screen either: the link was refused, not held.
    expect(state().pairTarget).toBeNull();
    expect(state().pairStep).toBeNull();
    expect(state().hostKey).toBe(PAIRED);
    confirm.mockRestore();
  });

  it("does not ask when the link is for the host already paired", () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);

    state().pairFromLink(LINK(PAIRED));

    expect(confirm).not.toHaveBeenCalled();
    expect(connect).toHaveBeenCalledOnce();
    confirm.mockRestore();
  });
});

describe("a link that launched the app and was declined", () => {
  it("leaves the phone on the host it is already paired with", async () => {
    // The app was opened BY the link, so `pairFromLink` had nothing to compare
    // it against — the key on disk is read by `init`, which asks there instead.
    // A "no" drops the link and nothing else: it is the reason the app is open,
    // so stopping would leave Home rendered against no connection at all.
    localStorage.setItem(
      "fletch-mobile-settings",
      JSON.stringify({
        host: "10.0.0.5",
        port: 47285,
        hostName: "Alex's Mac mini",
        hostKey: PAIRED,
      }),
    );
    const targets: HostTarget[] = [];
    useStore.setState({
      ready: false,
      hostKey: null,
      pairStep: null,
      pairTarget: null,
      connect: async (target: HostTarget) => {
        targets.push(target);
      },
    });
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    // Out of the suite's `?mock=1`, so `init` reaches the saved-host branch
    // rather than dialling the mock host ahead of it.
    const url = window.location.href;
    window.history.replaceState({}, "", "/");

    state().pairFromLink(LINK("YW5vdGhlcg"));
    expect(targets, "held until init has read what is on disk").toEqual([]);

    await state().init();

    expect(confirm).toHaveBeenCalledOnce();
    expect(targets).toHaveLength(1);
    expect(targets[0]).toMatchObject({ host: "10.0.0.5", hostKey: PAIRED });
    expect(state().pairTarget).toBeNull();
    window.history.replaceState({}, "", url);
    confirm.mockRestore();
  });
});
