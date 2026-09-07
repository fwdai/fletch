// WebSocket transport. Inside Tauri the socket is opened on the Rust side
// (tauri-plugin-websocket), which sidesteps iOS App Transport Security refusing
// a cleartext `ws://` from the webview. Outside Tauri (`bun run dev` in a
// desktop browser against a host on the LAN) the browser's own WebSocket is
// used, so the same UI is drivable without a device build.

import TauriWebSocket from "@tauri-apps/plugin-websocket";
import type { Socket, SocketFactory, SocketHandlers } from "./socket";

export const inTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const tauriSocket: SocketFactory = async (url, handlers) => {
  const ws = await TauriWebSocket.connect(url);
  ws.addListener((message) => {
    // The plugin models frames as a tagged union. Only Text carries protocol
    // frames; Close reports both a peer close and a dead transport (null
    // frame), and there is no separate error message — a failed connect
    // rejects `connect` instead.
    if (message.type === "Text") handlers.onMessage(message.data);
    else if (message.type === "Close") {
      handlers.onClose(message.data?.code ?? 1006, message.data?.reason);
    }
  });
  handlers.onOpen();
  return {
    send: (text) => ws.send(text),
    close: () => {
      void ws.disconnect().catch(() => {});
    },
  };
};

const browserSocket: SocketFactory = (url, handlers) =>
  new Promise<Socket>((resolve, reject) => {
    const ws = new WebSocket(url);
    let opened = false;
    ws.onopen = () => {
      opened = true;
      handlers.onOpen();
      resolve({
        send: (text) => ws.send(text),
        close: () => ws.close(),
      });
    };
    ws.onmessage = (e) => {
      if (typeof e.data === "string") handlers.onMessage(e.data);
    };
    ws.onerror = () => {
      if (!opened) reject(new Error(`Cannot reach ${url}`));
      else handlers.onError("socket error");
    };
    ws.onclose = (e) => {
      if (!opened) reject(new Error(e.reason || `Cannot reach ${url}`));
      else handlers.onClose(e.code, e.reason);
    };
  });

/** The transport for this runtime. Rust-side in Tauri, browser otherwise. */
export const openWebSocket: SocketFactory = (url: string, handlers: SocketHandlers) =>
  inTauri() ? tauriSocket(url, handlers) : browserSocket(url, handlers);
