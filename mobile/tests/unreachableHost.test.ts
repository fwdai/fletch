// A paired host that does not answer holds up none of the others. `init` dials
// every one at once, so the app is ready and usable on whichever answer, and
// the one that is down says so on its own store while its client retries.
//
// Its own file because `init` runs once per module registry.

import { describe, expect, it, vi } from "vitest";
import { registerMockHost } from "../src/remote/mock";
import { appStore, hostStoreOf } from "../src/store";
import { hostOnScreen, MINI, PAIRED_MINI, PAIRED_STUDIO, pairedOnDisk, STUDIO } from "./support";

const app = () => appStore.getState();
const mini = () => hostStoreOf(MINI).getState();
const studio = () => hostStoreOf(STUDIO).getState();

describe("a launch with one host asleep", () => {
  it("is ready on the others, and the one asleep connects when it wakes", async () => {
    registerMockHost(STUDIO, { unreachable: true });
    // The one asleep is the one the app was left on.
    const restoreUrl = pairedOnDisk([PAIRED_MINI, PAIRED_STUDIO], STUDIO);

    // Settles, rather than rejecting, with one dial failed.
    await app().init();

    expect(app().ready).toBe(true);
    expect(app().hostKey).toBe(STUDIO);
    expect(mini().connection).toBe("connected");
    expect(mini().workspace).not.toBeNull();
    // The failure is the studio's own, and nobody else's.
    expect(studio().connection).toBe("error");
    expect(studio().retrying).toBe(true);
    expect(studio().connectionError).toContain("No answer from your Mac");
    expect(mini().connectionError).toBeNull();
    expect(app().lastError).toBeNull();

    // Usable on the mini at once.
    await app().switchHost(MINI);
    expect(hostOnScreen().getState().connection).toBe("connected");

    // Awake again: its own retry brings it in, with nobody asking.
    registerMockHost(STUDIO, {});
    await vi.waitFor(() => expect(studio().connection).toBe("connected"), { timeout: 5000 });
    expect(studio().connectionError).toBeNull();
    expect(mini().connection).toBe("connected");
    restoreUrl();
  });
});
