import { beforeEach, describe, expect, it, vi } from "vitest";

const remote = vi.hoisted(() => ({
  remoteStatus: vi.fn(),
  remoteSetEnabled: vi.fn(),
  remoteBeginPairing: vi.fn(),
  remoteDevicePublicKey: vi.fn(),
}));
vi.mock("@/api/domains/remote", () => ({ remoteApi: remote }));
vi.mock("@/storage/remoteHosts", () => ({
  saveHost: vi.fn(async () => {}),
  loadHosts: vi.fn(async () => []),
  updateHost: vi.fn(async () => {}),
  forgetHost: vi.fn(async () => {}),
}));
// A runner create listens for its steps; nothing here emits any.
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("./device", () => ({
  thisDevice: async () => ({ name: "Studio", platform: "macos", appVersion: "0.8.29" }),
}));

import { api, type CloudRunnerRecord, type CloudRunnerStatus } from "@/api";
import { forgetHost, loadHosts, type SavedHost, saveHost } from "@/storage/remoteHosts";
import { useAppStore } from "@/store";
import { candidatesFor } from "./candidates";
import {
  addHost,
  followRunnerStatus,
  hosts,
  offerPairBack,
  pairWithOfferedLink,
  reconcileRunnerHost,
} from "./hosts";
import { localHostname, parseAddress } from "./pairing";

const RECORD = { hostKey: "mac-b", name: "Mac B", addr: "10.0.0.9:47285" };
const NEARBY = { host: "10.0.0.9", port: 47285, hostKey: "mac-b", confirm: true as const };
const LINK = { host: "10.0.0.9", port: 47285, hostKey: "mac-b", pairingToken: "ABCD2345" };
const INVITE = "fletch://pair?host=mac-a&addr=10.0.0.2:47285&token=K7PQ2M9X&name=Mac%20A";

let pair: ReturnType<typeof vi.spyOn>;
let supports: ReturnType<typeof vi.spyOn>;
let call: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  vi.clearAllMocks();
  useAppStore.getState().setPairBackOffer(null);
  pair = vi.spyOn(hosts, "pair").mockResolvedValue(RECORD);
  supports = vi.spyOn(hosts, "supports").mockReturnValue(true);
  call = vi.spyOn(hosts, "call").mockResolvedValue({});
  remote.remoteStatus.mockResolvedValue({ listening: true, error: null });
  remote.remoteSetEnabled.mockResolvedValue({ listening: true, error: null });
  remote.remoteBeginPairing.mockResolvedValue({ url: INVITE });
});

/** docs/remote-protocol.md, "Pairing back": Mac A's half. */
describe("after a confirmed pairing, this Mac asks whether the host may control it too", () => {
  it("asks when this Mac is a host and the host takes offers — and mints nothing yet", async () => {
    await addHost(NEARBY);
    expect(supports).toHaveBeenCalledWith("mac-b", "offer_invite");
    expect(useAppStore.getState().pairBackOffer).toEqual({
      hostKey: "mac-b",
      name: "Mac B",
      enables: false,
    });
    expect(remote.remoteBeginPairing).not.toHaveBeenCalled();
    expect(call).not.toHaveBeenCalled();
  });

  it("asks with remote control off too, saying Allow turns it on, and turns nothing on yet", async () => {
    remote.remoteStatus.mockResolvedValue({ listening: false, error: null });
    await addHost(NEARBY);
    expect(useAppStore.getState().pairBackOffer).toMatchObject({ enables: true });
    expect(remote.remoteSetEnabled).not.toHaveBeenCalled();
  });

  it("does not ask when this Mac cannot be a host: its device store is broken", async () => {
    remote.remoteStatus.mockResolvedValue({ listening: false, error: "can't store devices" });
    await addHost(NEARBY);
    expect(useAppStore.getState().pairBackOffer).toBeNull();
    remote.remoteStatus.mockRejectedValue(new Error("no engine"));
    await addHost(NEARBY);
    expect(useAppStore.getState().pairBackOffer).toBeNull();
  });

  it("does not ask a host that would refuse the offer (a Control pairing, an older host)", async () => {
    supports.mockReturnValue(false);
    await addHost(NEARBY);
    expect(remote.remoteStatus).not.toHaveBeenCalled();
    expect(useAppStore.getState().pairBackOffer).toBeNull();
  });

  it("does not ask after a pairing with a code", async () => {
    await addHost(LINK);
    expect(pair).toHaveBeenCalledWith(LINK, undefined);
    expect(useAppStore.getState().pairBackOffer).toBeNull();
  });

  it("on Allow, mints a Full link here and hands it to that host", async () => {
    await offerPairBack("mac-b");
    expect(remote.remoteSetEnabled).not.toHaveBeenCalled();
    expect(remote.remoteBeginPairing).toHaveBeenCalledWith("full");
    expect(call).toHaveBeenCalledWith("mac-b", "offer_invite", { url: INVITE });
  });

  it("on Allow with remote control off, turns it on first", async () => {
    remote.remoteStatus.mockResolvedValue({ listening: false, error: null });
    await offerPairBack("mac-b");
    expect(remote.remoteSetEnabled).toHaveBeenCalledWith(true);
    expect(call).toHaveBeenCalledWith("mac-b", "offer_invite", { url: INVITE });
  });

  it("hands nothing over when remote control will not come up", async () => {
    remote.remoteStatus.mockResolvedValue({ listening: false, error: null });
    remote.remoteSetEnabled.mockResolvedValue({ listening: false, error: "port in use" });
    await expect(offerPairBack("mac-b")).rejects.toThrow("port in use");
    expect(remote.remoteBeginPairing).not.toHaveBeenCalled();
    expect(call).not.toHaveBeenCalled();
  });

  it("rejects with the host's words when it refuses", async () => {
    call.mockRejectedValue(new Error("forbidden"));
    await expect(offerPairBack("mac-b")).rejects.toThrow("forbidden");
  });
});

/** Pairing again with a host that is saved — an expired link pasted for it, or
 *  one that changes its access — first takes its live connection down. */
describe("a failed pairing with a host already saved", () => {
  it("dials the host again from its saved record", async () => {
    const saved = { ...RECORD, pairedAt: "2026-01-01T00:00:00Z" };
    vi.mocked(loadHosts).mockResolvedValue([saved]);
    pair.mockRejectedValue(new Error("That pairing code didn't work."));
    const adopt = vi.spyOn(hosts, "adopt").mockResolvedValue();

    await expect(addHost(LINK)).rejects.toThrow("didn't work");
    expect(adopt).toHaveBeenCalledWith(saved);
  });

  it("leaves nothing behind for a host that was never saved", async () => {
    vi.mocked(loadHosts).mockResolvedValue([]);
    pair.mockRejectedValue(new Error("That pairing code didn't work."));
    const adopt = vi.spyOn(hosts, "adopt").mockResolvedValue();

    await expect(addHost(LINK)).rejects.toThrow("didn't work");
    expect(adopt).not.toHaveBeenCalled();
  });
});

/** The cloud runner this Mac created: the engine authorised this device's key
 *  on it, and it is reached through its relay alone. */
describe("the cloud runner's saved host follows the runner's status", () => {
  // A 32-byte host key, base64url.
  const KEY = "aG9zdC1rZXktMDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3";
  const RUNNER: CloudRunnerRecord = {
    hostId: KEY,
    name: "Studio runner",
    relay: "wss://relay.example",
  };
  const ADDR = localHostname(KEY) ?? "";
  const running = (record = RUNNER): CloudRunnerStatus => ({
    state: "running",
    provider: "fly",
    record,
  });
  const none = (): CloudRunnerStatus => ({ state: "none", provider: null, record: null });
  /** The runner's host as saved: marked as the runner's. */
  const marked = (over: Partial<SavedHost> = {}): SavedHost => ({
    hostKey: KEY,
    name: "Studio runner",
    addr: ADDR,
    relay: "wss://relay.example",
    pairedAt: "2026-11-19T00:00:00Z",
    runner: true,
    ...over,
  });
  const HAND: SavedHost = { ...RECORD, pairedAt: "2026-01-01T00:00:00Z" };

  let adopt: ReturnType<typeof vi.spyOn>;
  beforeEach(() => {
    adopt = vi.spyOn(hosts, "adopt").mockResolvedValue();
  });

  it("a runner with no saved host gets one, marked, dialled with no pairing under its key's name", async () => {
    vi.mocked(loadHosts).mockResolvedValue([HAND]);
    await reconcileRunnerHost(running());
    expect(ADDR).toMatch(/^fletch-[0-9a-f]{16}\.local$/);
    const record = {
      hostKey: KEY,
      name: "Studio runner",
      relay: "wss://relay.example",
      addr: ADDR,
    };
    expect(saveHost).toHaveBeenCalledWith({
      ...record,
      pairedAt: expect.any(String),
      runner: true,
    });
    expect(adopt).toHaveBeenCalledWith(record);
    expect(pair).not.toHaveBeenCalled();
    expect(forgetHost).not.toHaveBeenCalled();
  });

  it("is left as it is once saved, and put back after it was forgotten in Settings", async () => {
    vi.mocked(loadHosts).mockResolvedValue([marked()]);
    await reconcileRunnerHost(running());
    expect(saveHost).not.toHaveBeenCalled();
    // Adopted all the same: nothing for a host already live.
    expect(adopt).toHaveBeenCalledWith(marked());

    vi.mocked(loadHosts).mockResolvedValue([]);
    await reconcileRunnerHost(running());
    expect(saveHost).toHaveBeenCalledTimes(1);
  });

  it("an adoption that failed is tried again by the next status answer, the host saved once", async () => {
    const savedRows: SavedHost[] = [];
    vi.mocked(loadHosts).mockImplementation(async () => [...savedRows]);
    vi.mocked(saveHost).mockImplementation(async (host) => {
      savedRows.push(host);
      return [...savedRows];
    });
    adopt.mockRejectedValueOnce(new Error("this device's key could not be read"));

    await expect(reconcileRunnerHost(running())).rejects.toThrow("could not be read");
    expect(saveHost).toHaveBeenCalledTimes(1);

    await reconcileRunnerHost(running());
    expect(saveHost).toHaveBeenCalledTimes(1);
    expect(adopt).toHaveBeenCalledTimes(2);
    expect(adopt).toHaveBeenLastCalledWith(expect.objectContaining({ hostKey: KEY }));
  });

  it("is forgotten when the status says there is no runner; a hand-paired host is not", async () => {
    vi.mocked(loadHosts).mockResolvedValue([marked(), HAND]);
    const forget = vi.spyOn(hosts, "forget");
    await reconcileRunnerHost(none());
    expect(forgetHost).toHaveBeenCalledTimes(1);
    expect(forgetHost).toHaveBeenCalledWith(KEY);
    expect(forget).toHaveBeenCalledWith(KEY);
    expect(forget).not.toHaveBeenCalledWith("mac-b");
  });

  it("never touches a host paired by hand, even one under the runner's key", async () => {
    vi.mocked(loadHosts).mockResolvedValue([{ ...marked(), runner: undefined }]);
    await reconcileRunnerHost(running());
    await reconcileRunnerHost(none());
    expect(saveHost).not.toHaveBeenCalled();
    expect(forgetHost).not.toHaveBeenCalled();
  });

  it("replaces the host of a runner deleted and made again while this window missed it", async () => {
    vi.mocked(loadHosts).mockResolvedValue([marked({ hostKey: "an-earlier-runner" }), HAND]);
    await reconcileRunnerHost(running());
    expect(forgetHost).toHaveBeenCalledTimes(1);
    expect(forgetHost).toHaveBeenCalledWith("an-earlier-runner");
    expect(saveHost).toHaveBeenCalledWith(expect.objectContaining({ hostKey: KEY, runner: true }));
  });

  it("is left alone while a runner is being set up, or its set-up did not finish", async () => {
    vi.mocked(loadHosts).mockResolvedValue([marked()]);
    await reconcileRunnerHost({ state: "setting_up", provider: null, record: null });
    await reconcileRunnerHost({ state: "incomplete", provider: "fly", record: null });
    await reconcileRunnerHost({ state: "incomplete", provider: "fly", record: RUNNER });
    expect(saveHost).not.toHaveBeenCalled();
    expect(forgetHost).not.toHaveBeenCalled();
  });

  it("at startup, a runner that exists with no saved host gets one", async () => {
    vi.mocked(loadHosts).mockResolvedValue([]);
    vi.spyOn(api, "cloudRunnerStatus").mockResolvedValue(running());
    await followRunnerStatus();
    expect(saveHost).toHaveBeenCalledWith(expect.objectContaining({ hostKey: KEY, runner: true }));
    expect(adopt).toHaveBeenCalledWith(expect.objectContaining({ hostKey: KEY }));
  });

  it("a status read that fails changes nothing", async () => {
    vi.mocked(loadHosts).mockResolvedValue([marked()]);
    vi.spyOn(api, "cloudRunnerStatus").mockRejectedValue("the provider is unreachable");
    await followRunnerStatus();
    await useAppStore
      .getState()
      .refreshCloudRunner()
      .catch(() => {});
    expect(saveHost).not.toHaveBeenCalled();
    expect(forgetHost).not.toHaveBeenCalled();
  });

  it("follows every status read: a create ends with the host saved, a delete with it gone", async () => {
    // The saved hosts as storage keeps them: what is saved is read back, and
    // what is forgotten is gone, so a status answered twice changes nothing
    // the second time.
    let rows: SavedHost[] = [HAND];
    vi.mocked(loadHosts).mockImplementation(async () => [...rows]);
    vi.mocked(saveHost).mockImplementation(async (host) => {
      rows = [...rows.filter((r) => r.hostKey !== host.hostKey), host];
      return rows;
    });
    vi.mocked(forgetHost).mockImplementation(async (key) => {
      rows = rows.filter((r) => r.hostKey !== key);
      return rows;
    });
    const status = vi.spyOn(api, "cloudRunnerStatus").mockResolvedValue(none());
    await followRunnerStatus();
    expect(forgetHost).not.toHaveBeenCalled();

    remote.remoteDevicePublicKey.mockResolvedValue("device-key");
    vi.spyOn(api, "cloudRunnerCreate").mockResolvedValue(RUNNER);
    status.mockResolvedValue(running());
    await useAppStore.getState().createCloudRunner("fly", "sk-ant-oat01-secret", "");
    await vi.waitFor(() =>
      expect(saveHost).toHaveBeenCalledWith(
        expect.objectContaining({ hostKey: KEY, runner: true }),
      ),
    );

    expect(saveHost).toHaveBeenCalledTimes(1);
    expect(rows.map((r) => r.hostKey)).toEqual([HAND.hostKey, KEY]);

    vi.spyOn(api, "cloudRunnerDelete").mockResolvedValue({ destroyed: [], warning: null });
    status.mockResolvedValue(none());
    await useAppStore.getState().deleteCloudRunner(false);
    await vi.waitFor(() => expect(forgetHost).toHaveBeenCalledWith(KEY));
    expect(forgetHost).toHaveBeenCalledTimes(1);
    expect(rows).toEqual([HAND]);
  });

  it("a create whose status read then fails still ends with the host saved and live", async () => {
    vi.mocked(loadHosts).mockResolvedValue([]);
    const status = vi.spyOn(api, "cloudRunnerStatus").mockResolvedValue(none());
    await followRunnerStatus();

    remote.remoteDevicePublicKey.mockResolvedValue("device-key");
    vi.spyOn(api, "cloudRunnerCreate").mockResolvedValue(RUNNER);
    status.mockRejectedValue("the provider is unreachable");
    await useAppStore.getState().createCloudRunner("fly", "sk-ant-oat01-secret", "");
    await vi.waitFor(() =>
      expect(saveHost).toHaveBeenCalledWith(
        expect.objectContaining({ hostKey: KEY, runner: true }),
      ),
    );
    await vi.waitFor(() =>
      expect(adopt).toHaveBeenCalledWith(expect.objectContaining({ hostKey: KEY })),
    );
  });

  it("dials the saved name first, which only the runner itself could answer, then the relay", () => {
    const address = parseAddress(ADDR);
    expect(address).not.toBeNull();
    const dials = candidatesFor({
      ...(address ?? { host: "", port: 0 }),
      hostKey: KEY,
      relay: RUNNER.relay,
    });
    expect(dials.map((d) => d.via)).toEqual(["lan", "relay"]);
    expect(dials[0].url).toBe(`ws://${localHostname(KEY)}:47285/ws`);
    expect(dials[0].alternates).toBeUndefined();
    expect(dials[1].url).toBe(`wss://relay.example/v1/device/${KEY}`);
  });
});

/** Mac B's half: the offer arrives from a Full device of this Mac. */
describe("an offered link is paired with like a pasted one", () => {
  it("pairs with the code it carries, and asks nothing back", async () => {
    await pairWithOfferedLink(INVITE);
    expect(pair).toHaveBeenCalledWith(
      expect.objectContaining({ hostKey: "mac-a", pairingToken: "K7PQ2M9X", name: "Mac A" }),
      undefined,
    );
    expect(useAppStore.getState().pairBackOffer).toBeNull();
  });

  it("refuses a link it cannot pair with, without dialling", async () => {
    await expect(pairWithOfferedLink("fletch://pair?addr=10.0.0.2:1")).rejects.toThrow(
      "no host key",
    );
    expect(pair).not.toHaveBeenCalled();
  });
});
