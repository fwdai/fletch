import { activeTransport, localTransport } from "./transport";

/** Call one engine op on the environment the UI is driving. For the local
 *  environment — the only one today — this is Tauri's `invoke` with one
 *  indirection in front of it (see ./transport). */
export function invoke<T>(op: string, args?: Record<string, unknown>): Promise<T> {
  return activeTransport().call<T>(op, args);
}

/** Call one op on the desktop itself, whatever environment is active: windows,
 *  editors, the mic, provider CLIs, this Mac's own remote-access server. These
 *  have no remote meaning, and a remote engine has no business answering them. */
export function invokeLocal<T>(op: string, args?: Record<string, unknown>): Promise<T> {
  return localTransport.call<T>(op, args);
}
