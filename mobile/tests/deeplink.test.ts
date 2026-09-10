// The launch URL is the whole point of this file. `onOpenUrl` is nothing but a
// listener for `deep-link://new-url`, which the plugin emits the moment iOS
// hands the URL over — on a cold start, long before the webview is there to
// hear it. A scanned QR that opens the app therefore arrives only through
// `getCurrent`, and an app that does not ask sits on the Pair screen doing
// nothing, which is exactly how this went wrong.

import { beforeEach, describe, expect, it, vi } from "vitest";

const { getCurrent, onOpenUrl, pairFromLink, delivered } = vi.hoisted(() => {
  const delivered: { cb: ((urls: string[]) => void) | null } = { cb: null };
  return {
    delivered,
    getCurrent: vi.fn<() => Promise<string[] | null>>(),
    onOpenUrl: vi.fn(async (cb: (urls: string[]) => void) => {
      delivered.cb = cb;
      return () => {};
    }),
    pairFromLink: vi.fn(),
  };
});

vi.mock("@tauri-apps/plugin-deep-link", () => ({ getCurrent, onOpenUrl }));
vi.mock("../src/store", () => ({ useStore: { getState: () => ({ pairFromLink }) } }));

const { registerDeepLinks } = await import("../src/deeplink");

const LINK =
  "fletch://pair?host=Zm9vYmFy&addr=192.168.1.24:47285&token=K7PQ2M9X&name=Alex%27s%20Mac";
const TARGET = {
  host: "192.168.1.24",
  port: 47285,
  hostKey: "Zm9vYmFy",
  pairingToken: "K7PQ2M9X",
  name: "Alex's Mac",
};

describe("pairing deep link", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    delivered.cb = null;
    getCurrent.mockResolvedValue(null);
    // `inTauri`: outside the app there is no plugin to register with.
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  });

  it("pairs from the URL the app was launched with, which no listener can hear", async () => {
    getCurrent.mockResolvedValue([LINK]);
    await registerDeepLinks();
    expect(pairFromLink).toHaveBeenCalledWith(TARGET);
  });

  it("subscribes before reading it, so a link arriving in between is not lost", async () => {
    await registerDeepLinks();
    expect(onOpenUrl.mock.invocationCallOrder[0]).toBeLessThan(
      getCurrent.mock.invocationCallOrder[0],
    );
  });

  it("pairs from a link tapped while the app is already running", async () => {
    await registerDeepLinks();
    expect(pairFromLink).not.toHaveBeenCalled();
    delivered.cb?.([LINK]);
    expect(pairFromLink).toHaveBeenCalledWith(TARGET);
  });

  it("ignores whatever else the app may have been opened with", async () => {
    getCurrent.mockResolvedValue(["https://fletch.sh", "fletch://pair?token=K7PQ2M9X"]);
    await registerDeepLinks();
    expect(pairFromLink).not.toHaveBeenCalled();
  });

  it("leaves manual pairing alone when the plugin is unavailable", async () => {
    onOpenUrl.mockRejectedValueOnce(new Error("no such plugin"));
    await expect(registerDeepLinks()).resolves.toBeUndefined();
    expect(getCurrent).not.toHaveBeenCalled();
  });
});
