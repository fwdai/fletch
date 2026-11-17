// Several paired hosts, one live at a time, over the real store and the real
// protocol client. Each host's link is built on the mock socket, which is
// chosen when the store module loads (the suite's `?mock=1`), so every host
// dialled answers as the mock host. The URL then leaves mock mode before
// `init`, so the store treats these as real hosts: it reads and writes the
// settings file, which is what upgrading, adding, switching and forgetting are
// about.
//
// One file, in order, because `init` runs once per module registry: the phone
// starts as the single-host build left it and goes from there.

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { ChatItem } from "../src/adapters";
import { linkOf } from "../src/host";
import type { HostTarget } from "../src/remote";
import { appStore, liveHost } from "../src/store";
import { loadSettings } from "../src/store/persist";

const app = () => appStore.getState();
const host = () => liveHost().getState();
const keys = () => app().hosts.map((h) => h.hostKey);
const savedKeys = async () => (await loadSettings()).hosts?.map((h) => h.hostKey);

/** Live on `hostKey`, with the connect that got there finished. */
const liveOn = (hostKey: string) =>
  vi.waitFor(
    () => {
      expect(app().hostKey).toBe(hostKey);
      expect(host().connection).toBe("connected");
      expect(app().pairStep).toBeNull();
      expect(host().workspace).not.toBeNull();
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
  await app().init();
  await liveOn(MINI);
});

afterAll(() => window.history.replaceState({}, "", url));

describe("the first launch after updating from the single-host build", () => {
  it("greets the host it was paired with — hello, not a new pairing", async () => {
    const { client } = linkOf(MINI);
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

    app().pairFromLink(LINK);
    // The Pair screen comes up over Home to show the link and its progress.
    expect(app().addingHost).toBe(true);
    expect(app().pairTarget).toEqual(LINK);
    await liveOn(STUDIO);

    expect(confirm).not.toHaveBeenCalled();
    expect(app().addingHost).toBe(false);
    expect(keys()).toEqual([MINI, STUDIO]);
    expect(await savedKeys()).toEqual([MINI, STUDIO]);
    expect((await loadSettings()).activeHostKey).toBe(STUDIO);
    // The code is spent: what a reconnect would use greets with hello.
    expect(linkOf(STUDIO).client.target?.pairingToken).toBeUndefined();
    confirm.mockRestore();
  });
});

describe("switching hosts", () => {
  it("drops what was on screen and greets the chosen host with hello", async () => {
    const studioLog: ChatItem[] = [{ kind: "user_message", text: "on the studio", turnId: "t" }];
    liveHost().setState({ logs: { arabia: studioLog } });
    const connect = vi.spyOn(linkOf(MINI).client, "connect");

    const switching = app().switchHost(MINI);
    // Agent ids recur across hosts: nothing of the studio's may show while the
    // mini answers.
    expect(app().hostKey).toBe(MINI);
    expect(host().logs).toEqual({});
    expect(host().workspace).toBeNull();
    await switching;
    await liveOn(MINI);

    expect(connect).toHaveBeenCalledOnce();
    expect(connect.mock.calls[0][0]).toMatchObject({ host: "10.0.0.5", hostKey: MINI });
    expect(connect.mock.calls[0][0].pairingToken).toBeUndefined();
    // One host live at a time: the studio's connection went with the switch.
    expect(linkOf(STUDIO).client.state).toBe("disconnected");
    expect((await loadSettings()).activeHostKey).toBe(MINI);
    expect(keys()).toEqual([MINI, STUDIO]);
    connect.mockRestore();
  });

  it("leaves the live host alone when it is picked again", async () => {
    const connect = vi.spyOn(linkOf(MINI).client, "connect");
    await app().switchHost(MINI);
    expect(connect).not.toHaveBeenCalled();
    connect.mockRestore();
  });

  it("can still switch away from a host that does not answer", async () => {
    const connect = vi
      .spyOn(linkOf(STUDIO).client, "connect")
      .mockRejectedValueOnce(new Error("Can't reach the Mac"));
    await expect(app().switchHost(STUDIO)).rejects.toThrow("Can't reach the Mac");
    expect(app().hostKey).toBe(STUDIO);
    expect(host().connectionError).toBe("Can't reach the Mac");
    // Back on the studio, nothing it showed before the switch away comes back.
    expect(host().logs).toEqual({});

    await app().switchHost(MINI);
    await liveOn(MINI);
    connect.mockRestore();
  });

  it("follows a tapped alert from the other host to the agent it names", async () => {
    app().openFromPush({ hostId: STUDIO, agentId: "arabia", kind: "turn_complete" });
    await liveOn(STUDIO);
    await vi.waitFor(() => {
      const top = app().nav[app().nav.length - 1];
      expect(top.screen).toBe("agent");
      expect(top.props.agentId).toBe("arabia");
    });
    await app().switchHost(MINI);
    await liveOn(MINI);
  });
});

describe("forgetting hosts", () => {
  it("cancelling an addition goes back to the live host", async () => {
    app().addHost();
    expect(app().addingHost).toBe(true);
    // The live link stops while the Pair screen is up.
    expect(host().connection).toBe("disconnected");

    await app().cancelAddHost();
    expect(app().addingHost).toBe(false);
    await liveOn(MINI);
    expect(keys()).toEqual([MINI, STUDIO]);
  });

  it("forgets the live host and carries on with the next one", async () => {
    // Added back the Pair screen's way, by typing the code.
    app().addHost();
    await app().connect({ ...LINK, pairingToken: "ABCD2345" });
    await liveOn(STUDIO);
    expect(keys()).toEqual([MINI, STUDIO]);

    const seen: (string | null)[] = [];
    const off = appStore.subscribe((s) => seen.push(s.hostKey));
    await app().unpair();
    await liveOn(MINI);
    off();
    // Straight to the next host: an unpaired moment would flash the Pair screen.
    expect(seen).not.toContain(null);
    expect(keys()).toEqual([MINI]);
    expect(await savedKeys()).toEqual([MINI]);
    expect((await loadSettings()).activeHostKey).toBe(MINI);
  });

  it("forgetting the last one leaves the phone unpaired, with its preferences", async () => {
    await app().unpair();
    expect(app().hostKey).toBeNull();
    expect(keys()).toEqual([]);
    expect(host().connection).not.toBe("connected");
    const saved = await loadSettings();
    expect(saved.hosts).toEqual([]);
    expect(saved.activeHostKey).toBeUndefined();
    expect(saved.theme).toBe("light");
  });
});
