// A host's place in the recent lists follows its newest connection, never the
// counts alone: a host that got through under Open and was refused since the
// posture tightened is blocked now, and says so.

import { describe, expect, it } from "vitest";
import {
  ALLOWED_VIAS,
  canAllow,
  countsLabel,
  hostStanding,
  REFUSED_VIAS,
  viaLabel,
} from "./egressActivity";

const host = (last_via: string, allowed: number, refused: number) => ({
  last_via,
  allowed,
  refused,
});

describe("hostStanding", () => {
  it("follows the newest connection, not the counts", () => {
    // Allowed twelve times under Open, refused since: blocked.
    expect(hostStanding(host("host", 12, 3))).toBe("blocked");
    // Refused three times under Allow list, allowed since the posture opened: reached.
    expect(hostStanding(host("open", 1, 3))).toBe("reached");
    for (const via of ALLOWED_VIAS) expect(hostStanding(host(via, 1, 0))).toBe("reached");
    for (const via of REFUSED_VIAS) expect(hostStanding(host(via, 0, 1))).toBe("blocked");
  });

  it("reads a via it does not know as refused", () => {
    expect(hostStanding(host("mystery", 5, 0))).toBe("blocked");
    expect(viaLabel("mystery")).toBe("mystery");
  });
});

describe("row labels", () => {
  it("spells every via", () => {
    for (const via of [...ALLOWED_VIAS, ...REFUSED_VIAS]) {
      expect(viaLabel(via)).not.toBe(via);
    }
    expect(viaLabel("port")).toBe("port not allowed");
  });

  it("shows both sides of a mixed history, led by the standing side", () => {
    expect(countsLabel(host("open", 12, 0))).toBe("12 connections");
    expect(countsLabel(host("open", 12, 3))).toBe("12 connections, 3 refused");
    expect(countsLabel(host("host", 0, 3))).toBe("3 refused");
    expect(countsLabel(host("host", 1, 3))).toBe("3 refused, 1 connection before");
  });

  it("offers an allow where a list could change the outcome", () => {
    expect(canAllow(host("open", 1, 0))).toBe(true);
    expect(canAllow(host("host", 0, 1))).toBe(true);
    expect(canAllow(host("port", 0, 1))).toBe(false);
    expect(canAllow(host("listed", 1, 0))).toBe(false);
  });
});
