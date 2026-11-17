import { localTransport } from "./transport";

/** One engine op as a call: the shape every routed domain in ./domains is built
 *  over (see `makeApi`). */
export type Invoke = <T>(op: string, args?: Record<string, unknown>) => Promise<T>;

/** Call one op on the desktop itself: Tauri's `invoke` with one indirection in
 *  front of it (see ./transport). Every op of this desktop's own engine goes
 *  here, and so do the ones that have no remote meaning whichever engine is
 *  asking — windows, editors, the mic, provider CLIs, this Mac's own
 *  remote-access server. */
export function invokeLocal<T>(op: string, args?: Record<string, unknown>): Promise<T> {
  return localTransport.call<T>(op, args);
}
