// The settings file as a list of paired hosts. The migration is the part that
// matters most: a phone updated from the single-host build has its one host at
// the top level of the file, and must come up still paired with it — the
// device key is the credential, so there is nothing to re-pair, only a record
// to reshape.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { migrate, upsertHost } from "../src/store/persist";

const LS_KEY = "fletch-mobile-settings";

const LEGACY = {
  host: "192.168.1.24",
  port: 47999,
  hostName: "Alex's Mac mini",
  hostKey: "aG9zdC1vbmU",
  relay: "wss://relay.test",
  destParents: { aG9zdC1vbmU: "/Users/alex/code" },
  pushPermission: { aG9zdC1vbmU: "granted" as const },
  theme: "light" as const,
};

/** A persist module with nothing cached, reading whatever is in storage now. */
async function fresh() {
  vi.resetModules();
  return import("../src/store/persist");
}

beforeEach(() => localStorage.clear());

describe("migrating the single-host settings", () => {
  it("turns the old host into a one-entry list, active, with the rest kept", () => {
    const next = migrate(LEGACY);
    expect(next.hosts).toEqual([
      {
        hostKey: "aG9zdC1vbmU",
        name: "Alex's Mac mini",
        host: "192.168.1.24",
        port: 47999,
        relay: "wss://relay.test",
        pairedAt: expect.any(String),
      },
    ]);
    expect(next.activeHostKey).toBe("aG9zdC1vbmU");
    // Both per-host maps were already keyed by host, so they carry straight over.
    expect(next.destParents).toEqual(LEGACY.destParents);
    expect(next.pushPermission).toEqual(LEGACY.pushPermission);
    expect(next.theme).toBe("light");
    for (const legacy of ["host", "port", "hostName", "hostKey", "relay"]) {
      expect(next).not.toHaveProperty(legacy);
    }
  });

  it("fills in what an old record could leave out", () => {
    const next = migrate({ host: "studio.local", hostKey: "k" });
    expect(next.hosts?.[0]).toMatchObject({ name: "studio.local", port: 47285 });
    expect(next.hosts?.[0]).not.toHaveProperty("relay");
  });

  it("drops an old record with no host key: it was never a pairing", () => {
    expect(migrate({ host: "192.168.1.24", port: 47285, theme: "dark" })).toEqual({
      theme: "dark",
    });
  });

  it("leaves a file that already has a list alone, bar the stale fields", () => {
    const hosts = [
      { hostKey: "k", name: "Studio", host: "10.0.0.2", port: 1, pairedAt: "2026-01-01" },
    ];
    expect(migrate({ hosts, activeHostKey: "k", hostKey: "old", host: "10.0.0.9" })).toEqual({
      hosts,
      activeHostKey: "k",
    });
  });

  it("is what the app reads on its first launch after the update, and what it then writes", async () => {
    localStorage.setItem(LS_KEY, JSON.stringify(LEGACY));
    const persist = await fresh();

    const read = await persist.loadSettings();
    expect(read.hosts?.map((h) => h.hostKey)).toEqual(["aG9zdC1vbmU"]);
    expect(read.activeHostKey).toBe("aG9zdC1vbmU");

    // Any write puts the new shape on disk; the old fields do not come back.
    await persist.saveSettings({ theme: "dark" });
    const disk = JSON.parse(localStorage.getItem(LS_KEY) ?? "{}");
    expect(disk.hosts).toHaveLength(1);
    expect(disk).not.toHaveProperty("hostKey");
    expect(disk).not.toHaveProperty("host");
  });
});

describe("the host list", () => {
  const studio = { hostKey: "studio", name: "Studio Mac", host: "10.0.0.2", port: 47285 };
  const mini = { hostKey: "mini", name: "Mac mini", host: "10.0.0.3", port: 47285 };

  it("adds a new host at the end and updates a known one in place", () => {
    const one = upsertHost([], studio);
    const two = upsertHost(one, mini);
    expect(two.map((h) => h.hostKey)).toEqual(["studio", "mini"]);

    const renamed = upsertHost(two, { ...studio, name: "Studio", relay: "wss://r" });
    expect(renamed.map((h) => h.hostKey)).toEqual(["studio", "mini"]);
    expect(renamed[0]).toMatchObject({ name: "Studio", relay: "wss://r" });
    // Pairing a host again does not make it new.
    expect(renamed[0].pairedAt).toBe(one[0].pairedAt);
  });

  it("saves a host as the active one, and forgets it without touching the others", async () => {
    const persist = await fresh();
    await persist.saveHost(studio);
    await persist.saveHost(mini);
    expect((await persist.loadSettings()).activeHostKey).toBe("mini");

    // Forgetting the inactive host keeps the active one active…
    await persist.forgetHost("studio");
    let saved = await persist.loadSettings();
    expect(saved.hosts?.map((h) => h.hostKey)).toEqual(["mini"]);
    expect(saved.activeHostKey).toBe("mini");

    // …and forgetting the active one leaves none.
    await persist.forgetHost("mini");
    saved = await persist.loadSettings();
    expect(saved.hosts).toEqual([]);
    expect(saved.activeHostKey).toBeUndefined();
  });

  it("lands two writes fired together, rather than the later undoing the earlier", async () => {
    const persist = await fresh();
    await persist.saveHost(studio);
    // A handshake updating the relay while a second host is being saved.
    await Promise.all([
      persist.updateHost("studio", { relay: "wss://relay.test" }),
      persist.saveHost(mini),
      persist.saveSettings({ theme: "light" }),
    ]);
    const saved = JSON.parse(localStorage.getItem(LS_KEY) ?? "{}");
    expect(saved.hosts.map((h: { hostKey: string }) => h.hostKey)).toEqual(["studio", "mini"]);
    expect(saved.hosts[0].relay).toBe("wss://relay.test");
    expect(saved.theme).toBe("light");
  });

  it("does not bring back a forgotten host by updating it", async () => {
    const persist = await fresh();
    await persist.updateHost("studio", { relay: "wss://relay.test" });
    expect((await persist.loadSettings()).hosts).toBeUndefined();
  });
});
