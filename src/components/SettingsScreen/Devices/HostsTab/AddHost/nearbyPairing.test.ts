import { describe, expect, it } from "vitest";
import type { NearbyHost } from "@/remote/nearby";
import { NO_CONFIRMER_ERROR } from "@/remote/types";
import {
  afterFailure,
  listedNearby,
  nextSightings,
  pairTargetFor,
  unpairedNearby,
} from "./nearbyPairing";

// A well-formed key (0x0123456789abcdef then zeros, base64url).
const KEY = "ASNFZ4mrze8AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

const studio: NearbyHost = {
  name: "Studio",
  hostKey: KEY,
  port: 47285,
  addresses: ["192.168.1.24"],
};

describe("pairing with a nearby host", () => {
  it("offers only the hosts this Mac has not paired with", () => {
    const attic: NearbyHost = { name: "Attic", hostKey: "OTHER", port: 1 };
    expect(unpairedNearby([studio, attic], [{ hostKey: KEY }])).toEqual([attic]);
    expect(unpairedNearby([studio], [])).toEqual([studio]);
  });

  it("asks by confirmation first, pinning the key the host announced", () => {
    expect(pairTargetFor(studio)).toEqual({
      host: "192.168.1.24",
      port: 47285,
      hostKey: KEY,
      name: "Studio",
      confirm: true,
    });
  });

  it("sends the code its screen shows, tidied", () => {
    expect(pairTargetFor(studio, " k7pq 2m9x ")).toMatchObject({
      pairingToken: "K7PQ2M9X",
      hostKey: KEY,
    });
    expect(pairTargetFor(studio, "K7PQ2M9X")).not.toHaveProperty("confirm");
  });

  it("refuses a host whose key names nothing", () => {
    expect(pairTargetFor({ ...studio, hostKey: "AAAA" })).toBeNull();
  });

  it("switches to the code when the host has nobody to confirm", () => {
    expect(afterFailure(studio, false, NO_CONFIRMER_ERROR)).toEqual({
      step: { kind: "code", host: studio, sending: false },
      error: null,
    });
  });

  it("goes back to the list after any other refusal, and stays on a refused code", () => {
    expect(afterFailure(studio, false, "Declined on the Mac")).toEqual({
      step: { kind: "idle" },
      error: "Declined on the Mac",
    });
    expect(afterFailure(studio, true, "That code has expired")).toEqual({
      step: { kind: "code", host: studio, sending: false },
      error: "That code has expired",
    });
  });

  it("names the host in a refusal written for the device that asked", () => {
    expect(afterFailure(studio, false, "On your Mac, open Settings → Devices.").error).toBe(
      "On “Studio”, open Settings → Devices.",
    );
    expect(afterFailure(studio, true, "Your Mac accepted, but couldn't save.").error).toBe(
      "“Studio” accepted, but couldn't save.",
    );
  });

  it("keeps the host being paired on the list when a browse misses it", () => {
    const attic: NearbyHost = { name: "Attic", hostKey: "OTHER", port: 1 };
    const asking = { kind: "asking", host: studio, code: "482913" } as const;
    expect(listedNearby([attic], [], asking)).toEqual([attic, studio]);
    expect(listedNearby([attic, studio], [], asking)).toEqual([attic, studio]);
    expect(listedNearby([attic], [], { kind: "idle" })).toEqual([attic]);
  });

  it("holds a host through two missed browses, then lets it go", () => {
    const attic: NearbyHost = { name: "Attic", hostKey: "OTHER", port: 1 };
    const names = (rows: { host: NearbyHost }[]) => rows.map((r) => r.host.name);
    let seen = nextSightings([], [studio, attic]);
    expect(names(seen)).toEqual(["Attic", "Studio"]);
    seen = nextSightings(seen, [studio]);
    seen = nextSightings(seen, [studio]);
    expect(names(seen)).toEqual(["Attic", "Studio"]);
    seen = nextSightings(seen, [studio]);
    expect(names(seen)).toEqual(["Studio"]);
    // Heard again, a host starts over, as its announcement now describes it.
    const moved = { ...attic, port: 2 };
    expect(nextSightings(nextSightings(seen, [attic]), [moved])).toContainEqual({
      host: moved,
      missed: 0,
    });
  });
});
