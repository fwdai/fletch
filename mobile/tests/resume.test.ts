// Returning to the foreground, over the mock host. Its own file: the probe's
// timeout is driven with fake timers, which must not touch the scripted turns
// the other store tests are waiting on.

import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { api, client, RESUME_PROBE_TIMEOUT_MS, useStore } from "../src/store";

const state = () => useStore.getState();

beforeAll(async () => {
  await state().init();
  await vi.waitFor(() => expect(state().connection).toBe("connected"), { timeout: 5000 });
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const foreground = () => document.dispatchEvent(new Event("visibilitychange"));

describe("returning to the foreground", () => {
  it("refreshes in place when the host answers", async () => {
    const read = vi.spyOn(api, "getWorkspace");
    const reconnect = vi.spyOn(client, "reconnect");
    foreground();
    await vi.waitFor(() => expect(read).toHaveBeenCalled());
    // Give the probe time to settle: it answers well inside the budget.
    await new Promise((r) => setTimeout(r, 50));
    expect(reconnect).not.toHaveBeenCalled();
    expect(state().connection).toBe("connected");
  });

  it("reconnects when the host does not answer the probe in time", async () => {
    vi.useFakeTimers();
    // A socket iOS froze: the request goes out and nothing ever comes back.
    vi.spyOn(api, "getWorkspace").mockReturnValue(new Promise(() => {}));
    const reconnect = vi.spyOn(client, "reconnect").mockResolvedValue({
      host: client.host ?? { name: "", appVersion: "", os: "" },
      workspace: state().workspace,
      protocol: client.protocol ?? undefined,
    });
    foreground();
    await vi.advanceTimersByTimeAsync(RESUME_PROBE_TIMEOUT_MS - 1);
    expect(reconnect).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(2);
    expect(reconnect).toHaveBeenCalledTimes(1);
  });

  it("reconnects when the socket refuses the probe outright", async () => {
    // The other way a dead socket shows: the write itself fails, at once.
    vi.spyOn(api, "getWorkspace").mockRejectedValue(new Error("not connected"));
    const reconnect = vi.spyOn(client, "reconnect").mockResolvedValue({
      host: client.host ?? { name: "", appVersion: "", os: "" },
      workspace: state().workspace,
      protocol: client.protocol ?? undefined,
    });
    foreground();
    await vi.waitFor(() => expect(reconnect).toHaveBeenCalledTimes(1));
  });

  it("does nothing while not connected", async () => {
    const read = vi.spyOn(api, "getWorkspace");
    const connected = vi.spyOn(client, "state", "get").mockReturnValue("connecting");
    foreground();
    await new Promise((r) => setTimeout(r, 20));
    expect(read).not.toHaveBeenCalled();
    connected.mockRestore();
  });
});
