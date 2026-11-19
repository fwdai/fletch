import { describe, expect, it } from "vitest";
import type { CloudProviderStatus } from "@/api";
import { isolationOf, shownIsolation } from "./isolation";

const engine = (sandbox_engine: string | null) => ({ sandbox_engine });

const FLY: CloudProviderStatus = {
  id: "fly",
  label: "Fly.io",
  connected: true,
  account: "acme",
  account_label: "Organization",
  account_placeholder: "personal",
  token_placeholder: "FlyV1 fm2_…",
  token_help: "Run fly tokens create org in a terminal.",
  billing: "Fly.io bills its disk while the workspace exists.",
};

describe("shownIsolation", () => {
  it("shows where a workspace runs, or where a move is taking it", () => {
    expect(shownIsolation(engine("box"), null, null).kind).toBe("box");
    expect(shownIsolation(engine(null), null, null).kind).toBe("seatbelt");
    expect(shownIsolation(engine("sandbox-exec"), "box", null).kind).toBe("box");
    expect(shownIsolation(engine("box"), "host", null).kind).toBe("seatbelt");
  });

  it("keeps showing the target once the host has restamped the record", () => {
    // Mid-move, the record already names where the workspace is going.
    expect(shownIsolation(engine("box"), "box", null).kind).toBe("box");
    expect(shownIsolation(engine("sandbox-exec"), "host", null).kind).toBe("seatbelt");
  });
});

describe("a workspace at a cloud provider", () => {
  const record = { sandbox_engine: "box", cloud_provider: "fly" };

  it("is a box, named for its provider and its bill", () => {
    const iso = isolationOf(record, [FLY]);
    expect(iso).toEqual({
      kind: "box",
      icon: "cloud",
      title: "Machine at Fly.io",
      hint: "Its own machine; reaches any host. Fly.io bills its disk while the workspace exists.",
    });
    expect(shownIsolation(record, null, [FLY])).toEqual(iso);
  });

  it("is a cloud machine in neutral words until its provider is known", () => {
    const neutral = {
      kind: "box",
      icon: "cloud",
      title: "Cloud machine",
      hint: "Its own machine; reaches any host. The provider bills your account.",
    };
    // The providers not read yet, or read without this one.
    expect(isolationOf(record, null)).toEqual(neutral);
    expect(isolationOf({ ...record, cloud_provider: "daytona" }, [FLY])).toEqual(neutral);
  });

  it("leaves a workspace on this Mac as it was", () => {
    expect(isolationOf({ sandbox_engine: "box", cloud_provider: null }, [FLY]).title).toBe(
      "Linux VM",
    );
    expect(isolationOf(engine("sandbox-exec"), [FLY]).title).toBe("macOS sandbox");
  });
});
