// The paired-host records go in the settings table the rest of the app's
// preferences use, so the round trip under test is JSON in and out of one row.

import { beforeEach, describe, expect, it, vi } from "vitest";

const store = new Map<string, string>();

vi.mock("./settings", () => ({
  getSetting: (key: string) => Promise.resolve(store.get(key) ?? null),
  setSetting: (key: string, value: unknown) => {
    store.set(key, typeof value === "string" ? value : JSON.stringify(value));
    return Promise.resolve();
  },
}));

const { forgetHost, loadHosts, REMOTE_HOSTS_KEY, saveHost } = await import("./remoteHosts");

const HOST = {
  hostKey: "aaaa-host-key",
  name: "Cloud box",
  addr: "10.0.0.4:47285",
  relay: "wss://relay.fletch.sh",
  pairedAt: "2026-09-18T10:00:00Z",
};

describe("saved remote hosts", () => {
  beforeEach(() => store.clear());

  it("reads as none before anything is written", async () => {
    expect(await loadHosts()).toEqual([]);
  });

  it("round-trips a record through the settings row", async () => {
    await saveHost(HOST);

    expect(await loadHosts()).toEqual([HOST]);
    expect(JSON.parse(store.get(REMOTE_HOSTS_KEY) ?? "null")).toEqual([HOST]);
  });

  it("keeps a relay-less host's optional fields absent", async () => {
    const lan = { hostKey: "bbbb", name: "Mini", addr: "mac.local", pairedAt: HOST.pairedAt };
    await saveHost(lan);

    expect(await loadHosts()).toEqual([lan]);
  });

  it("replaces the record for a host paired again rather than listing it twice", async () => {
    await saveHost(HOST);
    await saveHost({ ...HOST, addr: "10.0.0.9:47285", name: "Cloud box 2" });

    const saved = await loadHosts();
    expect(saved).toHaveLength(1);
    expect(saved[0]).toMatchObject({ addr: "10.0.0.9:47285", name: "Cloud box 2" });
  });

  it("forgets one host and leaves its neighbours", async () => {
    await saveHost(HOST);
    await saveHost({ ...HOST, hostKey: "bbbb", name: "Mini" });

    expect(await forgetHost(HOST.hostKey)).toEqual([
      expect.objectContaining({ hostKey: "bbbb", name: "Mini" }),
    ]);
    expect(await loadHosts()).toHaveLength(1);
  });

  it("forgetting a host that was never saved is a no-op", async () => {
    await saveHost(HOST);

    expect(await forgetHost("never-paired")).toEqual([HOST]);
  });

  it("skips a corrupt entry instead of losing its neighbours", async () => {
    store.set(REMOTE_HOSTS_KEY, JSON.stringify([{ name: "no key" }, HOST, null, 7]));

    expect(await loadHosts()).toEqual([HOST]);
  });

  it("reads unparseable and non-array values as none", async () => {
    store.set(REMOTE_HOSTS_KEY, "{not json");
    expect(await loadHosts()).toEqual([]);

    store.set(REMOTE_HOSTS_KEY, JSON.stringify({ hostKey: "a" }));
    expect(await loadHosts()).toEqual([]);
  });
});
