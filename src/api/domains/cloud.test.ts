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

// The runner's commands take what `src-tauri/src/commands/cloud.rs` names:
// the create's inputs as one object, in the engine's camelCase.
describe("the runner's commands", () => {
  beforeEach(() => rawInvoke.mockReset().mockResolvedValue(null));

  it("sends the provider and the inputs to create", async () => {
    const inputs = {
      devicePublicKey: "device-key",
      deviceName: "Studio",
      devicePlatform: "macos",
      claudeToken: "sk-ant-oat01-secret",
      name: null,
    };
    await cloudApi.cloudRunnerCreate("fly", inputs);
    expect(rawInvoke).toHaveBeenCalledWith("cloud_runner_create", { provider: "fly", inputs });
  });

  it("sends whether a delete is forced, and no preset for a full pairing", async () => {
    await cloudApi.cloudRunnerDelete(true);
    expect(rawInvoke).toHaveBeenCalledWith("cloud_runner_delete", { force: true });
    await cloudApi.cloudRunnerPairingLink();
    expect(rawInvoke).toHaveBeenCalledWith("cloud_runner_pairing_link", { preset: null });
  });

  it("asks for the status, a start and a stop with nothing else", async () => {
    await cloudApi.cloudRunnerStatus();
    await cloudApi.cloudRunnerStart();
    await cloudApi.cloudRunnerStop();
    expect(rawInvoke.mock.calls).toEqual([
      ["cloud_runner_status", undefined],
      ["cloud_runner_start", undefined],
      ["cloud_runner_stop", undefined],
    ]);
  });
});
