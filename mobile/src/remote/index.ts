import { ProtocolClient } from "@desktop/remote/client";
import type { DeviceInfo, RemoteClient } from "@desktop/remote/types";
import { openWebSocket } from "@desktop/remote/ws";
import { mockSocket } from "./mock";

export {
  type Candidate,
  candidatesFor,
  LAN_OPEN_TIMEOUT_MS,
  RELAY_OPEN_TIMEOUT_MS,
} from "@desktop/remote/candidates";
export { ProtocolClient } from "@desktop/remote/client";
export { parseAddress, parsePairUrl, relayDeviceUrl, wsUrl } from "@desktop/remote/pairing";
export * from "@desktop/remote/types";

const search = () =>
  typeof window === "undefined"
    ? new URLSearchParams()
    : new URLSearchParams(window.location.search);

/** Mock mode: `VITE_FLETCH_MOCK=1`, or `?mock` / `?mock=1` in the URL. */
export function mockEnabled(): boolean {
  if (import.meta.env?.VITE_FLETCH_MOCK === "1") return true;
  return search().has("mock");
}

/** `?mockSpeed=0.05` runs the scripted stream 20× faster — what the tests use
 *  so they don't wait out demo-paced delays. */
const mockSpeed = () => {
  const raw = Number(search().get("mockSpeed"));
  return Number.isFinite(raw) && raw >= 0 && search().has("mockSpeed") ? raw : 1;
};

const DEVICE: DeviceInfo = {
  name:
    typeof navigator !== "undefined" && /iPhone|iPad/.test(navigator.userAgent)
      ? "iPhone"
      : "Fletch Mobile (browser)",
  platform:
    typeof navigator !== "undefined" && /iPhone|iPad/.test(navigator.userAgent) ? "ios" : "web",
  appVersion: "0.1.0",
};

export function createClient(mock = mockEnabled()): RemoteClient {
  return new ProtocolClient({
    openSocket: mock ? mockSocket({ speed: mockSpeed() }) : openWebSocket,
    device: DEVICE,
  });
}
