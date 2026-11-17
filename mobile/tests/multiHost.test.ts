// Several paired hosts, every one connected at once, over the real store and
// the real protocol client. Each host's link is built on the mock socket, which
// is chosen when the store module loads (the suite's `?mock=1`), so every host
// dialled answers as the mock host. The URL then leaves mock mode before
// `init`, so the store treats these as real hosts: it reads and writes the
// settings file, which is what upgrading, adding, switching and forgetting are
// about. Hosts that tell apart, content and all, are liveHosts.test's.
//
// One file, in order, because `init` runs once per module registry: the phone
// starts as the single-host build left it and goes from there.

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { ChatItem } from "../src/adapters";
import { linkOf } from "../src/host";
import type { HostTarget } from "../src/remote";
import { registerMockHost } from "../src/remote/mock";
import { appStore, hostStoreOf } from "../src/store";
import { loadSettings } from "../src/store/persist";
import { hostOnScreen, leaveMockMode, STUDIO } from "./support";

const app = () => appStore.getState();
const host = () => hostOnScreen().getState();
const keys = () => app().hosts.map((h) => h.hostKey);
const savedKeys = async () => (await loadSettings()).hosts?.map((h) => h.hostKey);

/** `hostKey` on screen, connected, with any pairing that got there finished. */
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

const LINK: HostTarget = {
  host: "10.0.0.7",
  port: 47285,
  hostKey: STUDIO,
  pairingToken: "K7PQ2M9X",
  name: "Studio Mac",
};

let restoreUrl: () => void;

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
  restoreUrl = leaveMockMode();
  await app().init();
  await liveOn(MINI);
});

afterAll(() => restoreUrl());

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
  it("adds it and puts it on screen, without asking and without dropping the first", async () => {
    const confirm = vi.spyOn(window, "confirm");
    const dropped = vi.spyOn(linkOf(MINI).client, "disconnect");

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
    // The first host stayed connected throughout, on its own link.
    expect(dropped).not.toHaveBeenCalled();
    expect(linkOf(MINI).client.state).toBe("connected");
    confirm.mockRestore();
    dropped.mockRestore();
  });
});

describe("switching hosts", () => {
  it("shows the chosen host at once, with nothing dropped or redialled", async () => {
    const studioLog: ChatItem[] = [{ kind: "user_message", text: "on the studio", turnId: "t" }];
    hostOnScreen().setState({ logs: { arabia: studioLog } });
    const connect = vi.spyOn(linkOf(MINI).client, "connect");
    const dropped = vi.spyOn(linkOf(STUDIO).client, "disconnect");

    const switching = app().switchHost(MINI);
    // Already connected, and kept current all along: on screen with its
    // workspace before anything is awaited.
    expect(app().hostKey).toBe(MINI);
    expect(host().connection).toBe("connected");
    expect(host().workspace).not.toBeNull();
    // Agent ids recur across hosts: what the studio holds stays the studio's.
    expect(host().logs.arabia ?? []).not.toContainEqual(studioLog[0]);
    await switching;

    expect(connect).not.toHaveBeenCalled();
    expect(dropped).not.toHaveBeenCalled();
    // The studio is still connected, and keeps what it had, off screen.
    expect(linkOf(STUDIO).client.state).toBe("connected");
    expect(hostStoreOf(STUDIO).getState().logs.arabia).toEqual(studioLog);
    expect((await loadSettings()).activeHostKey).toBe(MINI);
    expect(keys()).toEqual([MINI, STUDIO]);
    connect.mockRestore();
    dropped.mockRestore();
  });

  it("leaves the host on screen alone when it is picked again", async () => {
    const connect = vi.spyOn(linkOf(MINI).client, "connect");
    await app().switchHost(MINI);
    expect(connect).not.toHaveBeenCalled();
    connect.mockRestore();
  });

  it("follows a tapped alert from the other host to the agent it names, dropping nothing", async () => {
    const dropped = vi.spyOn(linkOf(MINI).client, "disconnect");
    app().openFromPush({ hostId: STUDIO, agentId: "arabia", kind: "turn_complete" });
    // On screen at once: the studio was connected all along.
    expect(app().hostKey).toBe(STUDIO);
    const top = app().nav[app().nav.length - 1];
    expect(top.screen).toBe("agent");
    expect(top.props).toEqual({ agentId: "arabia", hostKey: STUDIO });
    await liveOn(STUDIO);
    expect(dropped).not.toHaveBeenCalled();
    expect(linkOf(MINI).client.state).toBe("connected");
    dropped.mockRestore();
    await app().switchHost(MINI);
    await liveOn(MINI);
  });

  it("dials a host that is down with no retry coming, and can still switch away from it", async () => {
    // Down for good — what a refused dial leaves — rather than retrying; and
    // asleep, so the dial goes unanswered.
    const { client } = linkOf(STUDIO);
    client.disconnect();
    registerMockHost(STUDIO, { unreachable: true });
    const connect = vi.spyOn(client, "connect");
    await expect(app().switchHost(STUDIO)).rejects.toThrow("No answer from your Mac");
    expect(connect).toHaveBeenCalledOnce();
    expect(connect.mock.calls[0][0]).toMatchObject({ host: "10.0.0.7", hostKey: STUDIO });
    expect(connect.mock.calls[0][0].pairingToken).toBeUndefined();
    expect(app().hostKey).toBe(STUDIO);
    expect(host().connectionError).toContain("No answer from your Mac");
    // The client's own retry carries on from here.
    expect(host().retrying).toBe(true);

    await app().switchHost(MINI);
    await liveOn(MINI);
    // Awake again: picked again, it is left to that retry, which lands.
    registerMockHost(STUDIO, {});
    await app().switchHost(STUDIO);
    expect(connect).toHaveBeenCalledOnce();
    await liveOn(STUDIO);
    connect.mockRestore();
    await app().switchHost(MINI);
    await liveOn(MINI);
  });
});

describe("forgetting hosts", () => {
  it("cancelling an addition goes back to the host on screen, connected all along", async () => {
    const connect = vi.spyOn(linkOf(MINI).client, "connect");
    app().addHost();
    expect(app().addingHost).toBe(true);
    // The Pair screen dials nothing until asked; the paired hosts stay up.
    expect(linkOf(MINI).client.state).toBe("connected");
    expect(linkOf(STUDIO).client.state).toBe("connected");

    app().cancelAddHost();
    expect(app().addingHost).toBe(false);
    await liveOn(MINI);
    expect(connect).not.toHaveBeenCalled();
    expect(keys()).toEqual([MINI, STUDIO]);
    connect.mockRestore();
  });

  it("forgets the host on screen and carries on with the next one", async () => {
    // Paired again the Pair screen's way, by typing the code.
    app().addHost();
    await app().connect({ ...LINK, pairingToken: "ABCD2345" });
    await liveOn(STUDIO);
    expect(keys()).toEqual([MINI, STUDIO]);
    const studio = linkOf(STUDIO);

    const seen: (string | null)[] = [];
    const off = appStore.subscribe((s) => seen.push(s.hostKey));
    await app().unpair(STUDIO);
    await liveOn(MINI);
    off();
    // Straight to the next host: an unpaired moment would flash the Pair screen.
    expect(seen).not.toContain(null);
    expect(keys()).toEqual([MINI]);
    expect(await savedKeys()).toEqual([MINI]);
    expect((await loadSettings()).activeHostKey).toBe(MINI);
    // Its link went with it.
    expect(studio.client.state).toBe("disconnected");
    expect(linkOf(STUDIO)).not.toBe(studio);
  });

  it("forgetting the last one leaves the phone unpaired, with its preferences", async () => {
    const mini = linkOf(MINI);
    await app().unpair(MINI);
    expect(app().hostKey).toBeNull();
    expect(keys()).toEqual([]);
    expect(mini.client.state).toBe("disconnected");
    const saved = await loadSettings();
    expect(saved.hosts).toEqual([]);
    expect(saved.activeHostKey).toBeUndefined();
    expect(saved.theme).toBe("light");
  });
});
