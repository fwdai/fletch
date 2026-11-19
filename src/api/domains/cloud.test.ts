// `cloud_connect` names what the token acts for `account`, whatever the
// provider calls it, and sends an empty one for the provider's default.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { cloudApi } from "./cloud";

const { rawInvoke } = vi.hoisted(() => ({ rawInvoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: rawInvoke }));

describe("cloudConnect", () => {
  beforeEach(() => rawInvoke.mockReset().mockResolvedValue(null));

  it("sends the token and the account", async () => {
    await cloudApi.cloudConnect("fly", "FlyV1 secret", "acme");
    expect(rawInvoke).toHaveBeenCalledWith("cloud_connect", {
      provider: "fly",
      token: "FlyV1 secret",
      account: "acme",
    });
  });

  it("sends an empty account for the provider's default", async () => {
    await cloudApi.cloudConnect("fly", "FlyV1 secret");
    expect(rawInvoke).toHaveBeenCalledWith("cloud_connect", {
      provider: "fly",
      token: "FlyV1 secret",
      account: "",
    });
  });
});
