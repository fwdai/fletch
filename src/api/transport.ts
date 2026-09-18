// The one module that touches Tauri's IPC. Everything above it speaks
// `Transport`: `call` for one request/response op, `on` for a backend event
// stream.
//
// `LocalTransport` is today's code behind that interface — `invoke`, and
// `listen` with the event envelope unwrapped — so the desktop's own engine is
// reached with one indirection and nothing else: no handshake, no allowlist, no
// serialisation of its own, all 212 commands still reachable
// (docs/multi-host-plan.md §5.1). `RemoteTransport` puts the same two methods on
// the protocol client in src/remote, which is how a second engine (an
// "environment") becomes reachable without a single caller changing.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { RemoteClient } from "@/remote/types";
import { activeEnvironment } from "@/store/environments";

/** Re-exported so callers of `Transport.on` (and of Tauri's own webview
 *  listeners) need no `@tauri-apps` import of their own. */
export type { UnlistenFn };

export interface Transport {
  call<T>(op: string, args?: Record<string, unknown>): Promise<T>;
  on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn>;
}

/** The desktop's in-process engine, over Tauri IPC. */
class LocalTransport implements Transport {
  call<T>(op: string, args?: Record<string, unknown>): Promise<T> {
    return invoke<T>(op, args);
  }

  on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
    return listen<T>(event, (e) => cb(e.payload));
  }
}

/** One instance for the whole app: the local environment is always present and
 *  always connected, so there is nothing per-environment to hold. */
export const localTransport: Transport = new LocalTransport();

/** A paired host, over the Noise-secured protocol client. Holds only what it
 *  needs from the client so a fake satisfies it in tests; the client itself is
 *  unchanged — it already speaks `call(op, args)` and `on(event, cb)`. */
export class RemoteTransport implements Transport {
  constructor(private readonly client: Pick<RemoteClient, "call" | "on">) {}

  call<T>(op: string, args: Record<string, unknown> = {}): Promise<T> {
    return this.client.call<T>(op, args);
  }

  /** The client subscribes synchronously and hands back an unsubscribe; the
   *  interface is async because Tauri's `listen` is. */
  on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
    return Promise.resolve(this.client.on(event, (payload) => cb(payload as T)));
  }
}

/** How ops and events reach the environment the UI is currently driving. The
 *  local environment carries no transport of its own — it is this singleton. */
export function activeTransport(): Transport {
  return activeEnvironment().transport ?? localTransport;
}

/** Raw Tauri IPC, for the one call whose body is not a record of arguments:
 *  `save_pasted_attachment` sends the bytes as the request body and the file
 *  name as a header. A remote environment uploads through `attachment_*`
 *  instead (docs/multi-host-plan.md §5.3, item 9), so this stays local. */
export const invokeLocalRaw = invoke;
