// The secure transport, which is a thin shell over the app's own Rust layer:
// `remote_connect` opens the WebSocket, runs the Noise handshake and reports
// the host's identity key, then every frame travels encrypted. This side only
// ever sees plaintext JSON (docs/remote-protocol.md, "Secure channel").
//
// Every connection has an id the Rust layer hands back and stamps on every
// event, and `remote_send`/`remote_close` take it. This wrapper only ever acts
// on, and only ever hears, its own connection — so a wrapper that was
// superseded while its handshake was in flight cannot close or spoof its
// successor, whatever order the two `invoke`s land in.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Socket, SocketFactory } from "./socket";

export const inTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

interface ConnectResult {
  hostKey: string;
  connectionId: number;
}

type RemoteEvent =
  | { kind: "message"; connectionId: number; text: string }
  | { kind: "close"; connectionId: number; code: number; reason: string }
  | { kind: "error"; connectionId: number; message: string };

const secureSocket: SocketFactory = async (url, handlers, opts) => {
  const subscriptions: UnlistenFn[] = [];
  let done = false;
  const stop = () => {
    for (const off of subscriptions) off();
    subscriptions.length = 0;
  };

  // Subscribe before connecting: the reader task starts as soon as the
  // handshake completes, so a frame can arrive before `invoke` resolves — and
  // before this wrapper knows its own id. Those are held back and replayed
  // once the id is known; anything stamped with another id is not ours.
  let id: number | null = null;
  const early: RemoteEvent[] = [];
  const deliver = (ev: RemoteEvent) => {
    if (ev.connectionId !== id || done) return;
    if (ev.kind === "message") handlers.onMessage(ev.text);
    else if (ev.kind === "error") handlers.onError(ev.message);
    else {
      done = true;
      stop();
      handlers.onClose(ev.code, ev.reason || undefined);
    }
  };
  const onEvent = (ev: RemoteEvent) => {
    if (id === null) early.push(ev);
    else deliver(ev);
  };
  subscriptions.push(
    await listen<{ connectionId: number; text: string }>("remote:message", (e) =>
      onEvent({ kind: "message", ...e.payload }),
    ),
    await listen<{ connectionId: number; code: number; reason: string }>("remote:close", (e) =>
      onEvent({ kind: "close", ...e.payload }),
    ),
    await listen<{ connectionId: number; message: string }>("remote:error", (e) =>
      onEvent({ kind: "error", ...e.payload }),
    ),
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
  id = result.connectionId;
  handlers.onOpen();
  for (const ev of early.splice(0)) deliver(ev);

  const socket: Socket = {
    hostKey: result.hostKey,
    send: (text) => invoke<void>("remote_send", { connectionId: id, text }),
    close: () => {
      done = true;
      stop();
      void invoke("remote_close", { connectionId: id }).catch(() => {});
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
