// Several paired hosts, one live at a time, over the real store and the real
// protocol client. The client is built on the mock socket when the store
// module loads (the suite's `?mock=1`), and every host it dials answers as the
// mock host. The URL then leaves mock mode before `init`, so the store treats
// these as real hosts: it reads and writes the settings file, which is what
// upgrading, adding, switching and forgetting are about.
//
// One file, in order, because `init` runs once per module registry: the phone
// starts as the single-host build left it and goes from there.

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { ChatItem } from "../src/adapters";
import type { HostTarget } from "../src/remote";
import { client, useStore } from "../src/store";
import { loadSettings } from "../src/store/persist";

const state = () => useStore.getState();
const keys = () => state().hosts.map((h) => h.hostKey);
const savedKeys = async () => (await loadSettings()).hosts?.map((h) => h.hostKey);

/** Live on `hostKey`, with the connect that got there finished. */
const liveOn = (hostKey: string) =>
  vi.waitFor(
    () => {
      expect(state().hostKey).toBe(hostKey);
      expect(state().connection).toBe("connected");
      expect(state().pairStep).toBeNull();
      expect(state().workspace).not.toBeNull();
    },
    { timeout: 5000 },
  );

/** The host the single-host build was paired with. */
const MINI = "aG9zdC1vbmU";
const STUDIO = "c3R1ZGlv";

const LINK: HostTarget = {
  host: "10.0.0.7",
  port: 47285,
  hostKey: STUDIO,
  pairingToken: "K7PQ2M9X",
  name: "Studio Mac",
};

const url = window.location.href;

beforeAll(async () => {
  localStorage.setItem(
    "fletch-mobile-settings",
    JSON.stringify({
      host: "10.0.0.5",
      port: 47285,
      hostName: "Alex's Mac mini",
      hostKey: MINI,
      theme: "light",
    }),
  );
  window.history.replaceState({}, "", "/");
  await state().init();
  await liveOn(MINI);
});

afterAll(() => window.history.replaceState({}, "", url));

describe("the first launch after updating from the single-host build", () => {
  it("greets the host it was paired with — hello, not a new pairing", async () => {
    expect(client.target).toMatchObject({ host: "10.0.0.5", port: 47285, hostKey: MINI });
    expect(client.target?.pairingToken).toBeUndefined();
    expect(client.target?.confirm).toBeUndefined();
    expect(keys()).toEqual([MINI]);
    const saved = await loadSettings();
    expect(saved.hosts).toEqual([expect.objectContaining({ hostKey: MINI, host: "10.0.0.5" })]);
    expect(saved.activeHostKey).toBe(MINI);
    expect(saved.theme).toBe("light");
  });
});

describe("adding a host from a pairing link", () => {
  it("adds it and makes it active, without asking and without dropping the first", async () => {
    const confirm = vi.spyOn(window, "confirm");

    state().pairFromLink(LINK);
    // The Pair screen comes up over Home to show the link and its progress.
    expect(state().addingHost).toBe(true);
    expect(state().pairTarget).toEqual(LINK);
    await liveOn(STUDIO);

    expect(confirm).not.toHaveBeenCalled();
    expect(state().addingHost).toBe(false);
    expect(keys()).toEqual([MINI, STUDIO]);
    expect(await savedKeys()).toEqual([MINI, STUDIO]);
    expect((await loadSettings()).activeHostKey).toBe(STUDIO);
    // The code is spent: what a reconnect would use greets with hello.
    expect(client.target?.pairingToken).toBeUndefined();
    confirm.mockRestore();
  });
});

describe("switching hosts", () => {
  it("drops what was on screen and greets the chosen host with hello", async () => {
    const studioLog: ChatItem[] = [{ kind: "user_message", text: "on the studio", turnId: "t" }];
    useStore.setState({ logs: { arabia: studioLog } });
    const connect = vi.spyOn(client, "connect");

    const switching = state().switchHost(MINI);
    // Agent ids recur across hosts: nothing of the studio's may show while the
    // mini answers.
    expect(state().hostKey).toBe(MINI);
    expect(state().logs).toEqual({});
    expect(state().workspace).toBeNull();
    await switching;
    await liveOn(MINI);

    expect(connect).toHaveBeenCalledOnce();
    expect(connect.mock.calls[0][0]).toMatchObject({ host: "10.0.0.5", hostKey: MINI });
    expect(connect.mock.calls[0][0].pairingToken).toBeUndefined();
    expect((await loadSettings()).activeHostKey).toBe(MINI);
    expect(keys()).toEqual([MINI, STUDIO]);
    connect.mockRestore();
  });

  it("leaves the live host alone when it is picked again", async () => {
    const connect = vi.spyOn(client, "connect");
    await state().switchHost(MINI);
    expect(connect).not.toHaveBeenCalled();
    connect.mockRestore();
  });

  it("can still switch away from a host that does not answer", async () => {
    const connect = vi
      .spyOn(client, "connect")
      .mockRejectedValueOnce(new Error("Can't reach the Mac"));
    await expect(state().switchHost(STUDIO)).rejects.toThrow("Can't reach the Mac");
    expect(state().hostKey).toBe(STUDIO);
    expect(state().connectionError).toBe("Can't reach the Mac");

    await state().switchHost(MINI);
    await liveOn(MINI);
    connect.mockRestore();
  });

  it("follows a tapped alert from the other host to the agent it names", async () => {
    state().openFromPush({ hostId: STUDIO, agentId: "arabia", kind: "turn_complete" });
    await liveOn(STUDIO);
    await vi.waitFor(() => {
      const top = state().nav[state().nav.length - 1];
      expect(top.screen).toBe("agent");
      expect(top.props.agentId).toBe("arabia");
    });
    await state().switchHost(MINI);
    await liveOn(MINI);
  });
});

describe("forgetting hosts", () => {
  it("forgets one that is not live without touching the live one", async () => {
    await state().forgetHost(STUDIO);
    expect(keys()).toEqual([MINI]);
    expect(await savedKeys()).toEqual([MINI]);
    expect(state().hostKey).toBe(MINI);
    expect(state().connection).toBe("connected");
  });

  it("cancelling an addition goes back to the live host", async () => {
    state().addHost();
    expect(state().addingHost).toBe(true);
    // The live link stops while the Pair screen is up.
    expect(state().connection).toBe("disconnected");

    await state().cancelAddHost();
    expect(state().addingHost).toBe(false);
    await liveOn(MINI);
    expect(keys()).toEqual([MINI]);
  });

  it("forgets the live host and carries on with the next one", async () => {
    // Added back the Pair screen's way, by typing the code.
    state().addHost();
    await state().connect({ ...LINK, pairingToken: "ABCD2345" });
    await liveOn(STUDIO);
    expect(keys()).toEqual([MINI, STUDIO]);

    const seen: (string | null)[] = [];
    const off = useStore.subscribe((s) => seen.push(s.hostKey));
    await state().forgetHost(STUDIO);
    await liveOn(MINI);
    off();
    // Straight to the next host: an unpaired moment would flash the Pair screen.
    expect(seen).not.toContain(null);
    expect(keys()).toEqual([MINI]);
    expect(await savedKeys()).toEqual([MINI]);
    expect((await loadSettings()).activeHostKey).toBe(MINI);
  });

  it("forgetting the last one leaves the phone unpaired, with its preferences", async () => {
    await state().unpair();
    expect(state().hostKey).toBeNull();
    expect(keys()).toEqual([]);
    expect(state().connection).not.toBe("connected");
    const saved = await loadSettings();
    expect(saved.hosts).toEqual([]);
    expect(saved.activeHostKey).toBeUndefined();
    expect(saved.theme).toBe("light");
  });
});
