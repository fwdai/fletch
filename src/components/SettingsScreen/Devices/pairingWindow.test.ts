import { describe, expect, it } from "vitest";
import { needsInvite, type WindowFacts } from "./pairingWindow";

const NOW = Date.parse("2026-11-10T12:00:00Z");

const live: WindowFacts = {
  canPair: true,
  invite: { expiresAt: "2026-11-10T12:04:00Z", preset: "full" },
  closed: false,
  preset: "full",
  redeemed: false,
  now: NOW,
};

describe("the Devices page's pairing window", () => {
  it("leaves a live invite alone", () => {
    expect(needsInvite(live)).toBe(false);
  });

  it("opens one when there is none", () => {
    expect(needsInvite({ ...live, invite: null })).toBe(true);
  });

  it("opens the next one when the invite lapses, or its expiry can't be read", () => {
    expect(needsInvite({ ...live, now: Date.parse("2026-11-10T12:04:00Z") })).toBe(true);
    expect(needsInvite({ ...live, invite: { expiresAt: "soon", preset: "full" } })).toBe(true);
  });

  it("opens the next one once a device has used the code", () => {
    expect(needsInvite({ ...live, redeemed: true })).toBe(true);
  });

  it("follows the preset chosen", () => {
    expect(needsInvite({ ...live, preset: "control" })).toBe(true);
  });

  it("opens nothing while this Mac can't pair", () => {
    expect(needsInvite({ ...live, canPair: false, invite: null })).toBe(false);
  });

  it("does not reopen a window the host closed after too many requests", () => {
    expect(needsInvite({ ...live, closed: true })).toBe(false);
    expect(needsInvite({ ...live, closed: true, now: NOW + 10 * 60_000 })).toBe(false);
  });
});
