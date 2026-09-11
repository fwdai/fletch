// Module-level state for in-app provider sign-ins, deliberately outside both
// React and the store: a login flow has to survive its terminal unmounting.
// Collapsing the provider row (or leaving Settings) mid-login must not abort
// the flow, so the output buffer and the exit result are held here and the
// terminal re-attaches to them on the next mount.
//
// The event taps are attached lazily on the first sign-in rather than in the
// app-wide `registerEventListeners`: nothing can arrive on these channels until
// a login is opened, and keeping them here makes the feature self-contained.

import type { ProviderLoginExitEvent } from "@/api";
import { api, onProviderLoginExit, onProviderLoginOutput } from "@/api";
import { createPtyChannel, type OutputHandler } from "@/pty/channel";
import { decodeBase64 } from "@/pty/decode";

/** Sign-in output per provider id. A login flow prints a URL, a code and a
 *  confirmation — 64 KiB is far more than enough to replay. */
const output = createPtyChannel(64 * 1024);

/** Everything a provider's sign-in has printed so far, to replay into a
 *  terminal that has just (re)mounted. */
export function readLoginBuffer(id: string): Uint8Array | undefined {
  return output.get(id);
}

export function registerLoginSink(id: string, handler: OutputHandler): () => void {
  return output.registerSink(id, handler);
}

/** Sign-ins opened in this app session, still running or already finished.
 *  What keeps a re-mount (the row was collapsed and re-expanded) from silently
 *  starting a second flow or wiping the outcome of the last one. Cleared only
 *  by an explicit `closeLogin`. */
const started = new Set<string>();

/** The subset of `started` still running. Frontend-side bookkeeping only — the
 *  backend is the authority and is idempotent either way. */
const live = new Set<string>();

/** The last exit of each provider's sign-in, kept so a terminal that mounts
 *  after the flow ended still shows the outcome. */
const exits = new Map<string, ProviderLoginExitEvent | undefined>();
const exitListeners = new Map<string, Set<(exit?: ProviderLoginExitEvent) => void>>();

export function getLoginExit(id: string): ProviderLoginExitEvent | undefined {
  return exits.get(id);
}

/** Subscribe to a provider's sign-in outcome; returns an unsubscribe fn. */
export function subscribeLoginExit(
  id: string,
  listener: (exit?: ProviderLoginExitEvent) => void,
): () => void {
  const set = exitListeners.get(id) ?? new Set();
  set.add(listener);
  exitListeners.set(id, set);
  return () => set.delete(listener);
}

function setExit(id: string, exit?: ProviderLoginExitEvent) {
  exits.set(id, exit);
  for (const listener of exitListeners.get(id) ?? []) listener(exit);
}

let taps: Promise<unknown> | undefined;

function ensureTaps() {
  taps ??= Promise.all([
    onProviderLoginOutput((e) => output.push(e.id, decodeBase64(e.bytes))),
    onProviderLoginExit((e) => {
      live.delete(e.id);
      setExit(e.id, e);
    }),
  ]);
  return taps;
}

/** Per-provider open promise, so keystrokes and the first resize (which xterm
 *  fires as soon as it fits its host) wait for the PTY to exist instead of
 *  racing it and being dropped. */
const opening = new Map<string, Promise<void>>();

/** Run a provider's sign-in from the top: clears what the last run printed and
 *  its outcome, so the terminal starts blank. This is "Sign in" on a provider
 *  that hasn't been signed in during this session, and "Run again" after one
 *  finished. */
export function runLogin(id: string, cols: number, rows: number): Promise<void> {
  ensureTaps();
  output.drop(id);
  setExit(id, undefined);
  started.add(id);
  live.add(id);
  const open = api.openProviderLogin(id, cols, rows).catch((err) => {
    // The open failed, so there is nothing to attach to: report it through the
    // same channel an exit uses and let the row offer "Run again".
    live.delete(id);
    setExit(id, { id, success: false, message: String(err) });
  });
  opening.set(id, open);
  return open;
}

/** The mount-time call: pick up a sign-in this session already opened, or start
 *  one if it hasn't. Re-attaching must not restart a flow still in progress,
 *  nor re-run one that already finished — the terminal replays the buffer and
 *  shows the recorded outcome instead. */
export function attachLogin(id: string, cols: number, rows: number) {
  if (started.has(id)) return;
  runLogin(id, cols, rows);
}

export async function writeLogin(id: string, data: string) {
  await opening.get(id);
  await api.writeProviderLogin(id, data).catch((err) => {
    console.error("writeProviderLogin failed", err);
  });
}

export async function resizeLogin(id: string, cols: number, rows: number) {
  await opening.get(id);
  // A resize that lands after the flow exited is expected, not an error.
  await api.resizeProviderLogin(id, cols, rows).catch(() => {});
}

/** Kill a provider's sign-in and forget it — the explicit Close only. The next
 *  "Sign in" then starts a fresh flow rather than re-attaching to this one. */
export async function closeLogin(id: string) {
  started.delete(id);
  live.delete(id);
  output.drop(id);
  setExit(id, undefined);
  opening.delete(id);
  await api.closeProviderLogin(id).catch((err) => {
    console.error("closeProviderLogin failed", err);
  });
}
