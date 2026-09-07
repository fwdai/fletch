// The secure transport, which is a thin shell over the app's own Rust layer:
// `remote_connect` opens the WebSocket, runs the Noise handshake and reports
// the host's identity key, then every frame travels encrypted. This side only
// ever sees plaintext JSON (docs/remote-protocol.md, "Secure channel").

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Socket, SocketFactory } from "./socket";

export const inTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

interface ConnectResult {
  hostKey: string;
}

const secureSocket: SocketFactory = async (url, handlers, opts) => {
  // Subscribe before connecting: the reader task starts as soon as the
  // handshake completes, so a frame can arrive before `invoke` resolves.
  const subscriptions: UnlistenFn[] = [];
  let done = false;
  const stop = () => {
    for (const off of subscriptions) off();
    subscriptions.length = 0;
  };
  subscriptions.push(
    await listen<{ text: string }>("remote:message", (e) => handlers.onMessage(e.payload.text)),
    await listen<{ code: number; reason: string }>("remote:close", (e) => {
      if (done) return;
      done = true;
      stop();
      handlers.onClose(e.payload.code, e.payload.reason || undefined);
    }),
    await listen<{ message: string }>("remote:error", (e) => handlers.onError(e.payload.message)),
  );
  let result: ConnectResult;
  try {
    result = await invoke<ConnectResult>("remote_connect", {
      url,
      hostKey: opts?.hostKey ?? null,
    });
  } catch (e) {
    stop();
    throw e instanceof Error ? e : new Error(String(e));
  }
  handlers.onOpen();
  const socket: Socket = {
    hostKey: result.hostKey,
    send: (text) => invoke<void>("remote_send", { text }),
    close: () => {
      done = true;
      stop();
      void invoke("remote_close").catch(() => {});
    },
  };
  return socket;
};

/** Outside Tauri there is no Rust layer to hold the device key or the Noise
 *  state, so a real host is unreachable — `bun run dev` in a browser is a
 *  mock-only dev loop. */
const noTransport: SocketFactory = (url) =>
  Promise.reject(
    new Error(
      `Cannot reach ${url}: the secure channel needs the app's Rust layer. ` +
        "In a browser, run with ?mock=1.",
    ),
  );

/** The transport for this runtime. */
export const openWebSocket: SocketFactory = (url, handlers, opts) =>
  inTauri() ? secureSocket(url, handlers, opts) : noTransport(url, handlers, opts);
