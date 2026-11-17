// Pairing again with a Mac this phone is already paired with — a fresh link to
// change its access, or an old one found in the photos. The pairing runs on
// that host's own link, so it takes the working connection over; whatever
// becomes of it, the host has to end up connected on its saved record again.
//
// Its own file because `init` runs once per module registry, and these start
// from hosts paired on disk.

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { linkOf } from "../src/host";
import { PAIRING_CODE_REFUSED } from "../src/remote";
import { registerMockHost } from "../src/remote/mock";
import { appStore, hostStoreOf } from "../src/store";
import { MINI, PAIRED_MINI, PAIRED_STUDIO, pairedOnDisk, STUDIO, settle } from "./support";

const app = () => appStore.getState();
const mini = () => linkOf(MINI).client;

/** Where a pairing link for the mini points: not where it was saved. */
const AWAY = { host: "192.168.1.99", port: 47285, hostKey: MINI, name: "Mac mini" };

let restoreUrl: () => void;

beforeAll(async () => {
  registerMockHost(MINI, {});
  registerMockHost(STUDIO, {});
  restoreUrl = pairedOnDisk([PAIRED_MINI, PAIRED_STUDIO], STUDIO);
  await app().init();
  await vi.waitFor(() => {
    expect(mini().state).toBe("connected");
    expect(linkOf(STUDIO).client.state).toBe("connected");
  });
});

afterAll(() => restoreUrl());

describe("pairing again with a paired Mac", () => {
  it("connects the host again on its saved record when the pairing fails", async () => {
    // The code has expired: the Mac hangs up on it.
    registerMockHost(MINI, { refusePairing: true });
    app().addHost();

    await expect(app().connect({ ...AWAY, pairingToken: "K7PQ2M9X" })).rejects.toThrow(
      PAIRING_CODE_REFUSED,
    );
    expect(app().pairError).toBe(PAIRING_CODE_REFUSED);

    // Back with no tap, greeting the saved address — not the link's, and not
    // with the dead code.
    await vi.waitFor(() => expect(mini().state).toBe("connected"));
    expect(mini().target).toMatchObject({ host: PAIRED_MINI.host, hostKey: MINI });
    expect(mini().target?.pairingToken).toBeUndefined();

    // And every Retry after it greets the host the same way.
    await hostStoreOf(MINI).getState().reconnect();
    expect(mini().state).toBe("connected");
    expect(mini().target?.pairingToken).toBeUndefined();

    app().cancelAddHost();
    registerMockHost(MINI, {});
  });

  it("leaves the host connected when a running re-pairing is cancelled, and the screen as it was", async () => {
    expect(app().hostKey).toBe(STUDIO);
    app().addHost();
    // Cancelled while the Mac decides: the code is on the phone's screen and
    // nobody has answered yet.
    const off = mini().onConfirmCode(() => setTimeout(() => app().cancelAddHost(), 0));

    await expect(app().connect({ ...AWAY, confirm: true })).rejects.toThrow();
    off();

    await vi.waitFor(() => expect(mini().state).toBe("connected"));
    expect(mini().target).toMatchObject({ host: PAIRED_MINI.host, hostKey: MINI });
    expect(mini().target?.confirm).toBeUndefined();
    // Well past the moment the Mac would have accepted: nothing lands.
    await settle(100);
    expect(app().hostKey).toBe(STUDIO);
    expect(app().addingHost).toBe(false);
    expect(app().pairError).toBeNull();
    expect(app().pairStep).toBeNull();
  });
});
