import { describe, expect, it } from "vitest";
import type { RemoteStatus } from "@/api";
import { connectionProblem } from "./connectionProblem";

const healthy: RemoteStatus = {
  enabled: true,
  listening: true,
  port: 47285,
  name: "Studio",
  hostId: "KEY",
  addresses: ["10.0.0.4"],
  devices: [],
  relay: { url: "wss://relay.fletch.sh", state: "connected", error: null },
  error: null,
  pairRequest: null,
};

describe("the Clients tab's connection warning", () => {
  it("says nothing when remote control is off or healthy", () => {
    expect(connectionProblem(null)).toBeNull();
    expect(connectionProblem({ ...healthy, enabled: false, listening: false })).toBeNull();
    expect(connectionProblem(healthy)).toBeNull();
    expect(
      connectionProblem({ ...healthy, relay: { ...healthy.relay, state: "connecting" } }),
    ).toBeNull();
  });

  it("names a listener that could not open its port", () => {
    expect(connectionProblem({ ...healthy, listening: false })).toMatch(/port could not be opened/);
  });

  it("names a Mac with no network address", () => {
    expect(connectionProblem({ ...healthy, addresses: [] })).toMatch(/No network address/);
  });

  it("surfaces the relay link's error, since the relay has no row of its own", () => {
    const relay = {
      url: "wss://relay.fletch.sh",
      state: "error" as const,
      error: "the relay stopped answering pings",
    };
    expect(connectionProblem({ ...healthy, relay })).toBe(
      "Devices off this network can't reach this Mac right now: the relay stopped answering pings",
    );
    expect(connectionProblem({ ...healthy, relay: { ...relay, error: null } })).toBe(
      "Devices off this network can't reach this Mac right now.",
    );
  });
});
