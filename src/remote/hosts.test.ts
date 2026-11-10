import { beforeEach, describe, expect, it, vi } from "vitest";

const remote = vi.hoisted(() => ({
  remoteStatus: vi.fn(),
  remoteSetEnabled: vi.fn(),
  remoteBeginPairing: vi.fn(),
}));
vi.mock("@/api/domains/remote", () => ({ remoteApi: remote }));
vi.mock("@/storage/remoteHosts", () => ({
  saveHost: vi.fn(async () => {}),
  loadHosts: vi.fn(async () => []),
  updateHost: vi.fn(async () => {}),
  forgetHost: vi.fn(async () => {}),
}));

import { useAppStore } from "@/store";
import { addHost, hosts, offerPairBack, pairWithOfferedLink } from "./hosts";

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
