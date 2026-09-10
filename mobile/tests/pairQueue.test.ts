// A pairing link and `init` start together — `main.tsx` renders the app and
// calls `registerDeepLinks` in the same breath — and the link has the less to
// wait on of the two. Acting on it early is what this file rules out: `init`
// finishes by reconnecting the saved host, which would supersede the pairing
// and spend a code that is single use, and it publishes the settings it read
// first, which on a fresh install means writing a null host key over the one
// a pairing that got in ahead of it had just pinned.
//
// Its own file because `init` runs at most once per module registry, and this
// is about what happens before it has finished.

import { describe, expect, it, vi } from "vitest";
import type { HostTarget } from "../src/remote";
import { useStore } from "../src/store";

const state = () => useStore.getState();

const LINK: HostTarget = {
  host: "192.168.1.24",
  port: 47285,
  hostKey: "Zm9vYmFy",
  pairingToken: "K7PQ2M9X",
  name: "Alex's Mac",
};

describe("a pairing link that arrives before init has finished", () => {
  it("is held, shown, and paired with once the persisted state is in", async () => {
    // Nothing may reach the client until `init` says the app is ready, so the
    // connection is the thing observed: `ready` as each call saw it.
    const readyAt: boolean[] = [];
    const targets: HostTarget[] = [];
    const connect = vi.fn(async (target: HostTarget) => {
      readyAt.push(state().ready);
      targets.push(target);
    });
    useStore.setState({ connect });

    state().pairFromLink(LINK);
    expect(connect).not.toHaveBeenCalled();
    expect(state().ready).toBe(false);
    // Held, but not invisible: the Pair screen shows the link and reads as
    // busy rather than offering a button that would start a second pairing.
    expect(state().pairTarget).toEqual(LINK);
    expect(state().pairStep).toBe("connecting");

    await state().init();

    // Once, with the link — not the mock host `init` would otherwise dial,
    // and not after being superseded by it.
    expect(targets).toEqual([LINK]);
    // And only after the settings on disk had been published, which is what
    // stops a fresh install's null host key landing on top of the pairing.
    expect(readyAt).toEqual([true]);
  });

  it("is spent: a second init has nothing held to pair with", async () => {
    const connect = vi.fn(async () => {});
    useStore.setState({ connect });
    await state().init();
    expect(connect).not.toHaveBeenCalled();
  });
});
