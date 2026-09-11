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

let taps: Promise<void> | undefined;

/** Attach the output and exit taps, once, shared by every caller. Resolves only
 *  after *both* listeners are live: a sign-in that prints or exits the moment
 *  it spawns (an unusable binary, or a CLI that is already signed in) would
 *  otherwise emit before anything is listening, leaving the terminal blank or
 *  the session running forever. A failed registration clears the memo so the
 *  next sign-in retries instead of inheriting the rejected promise. */
function ensureTaps(): Promise<void> {
  taps ??= Promise.all([
    onProviderLoginOutput((e) => output.push(e.id, decodeBase64(e.bytes))),
    onProviderLoginExit((e) => {
      // An exit during a Close is the flow being torn down — the kill landing,
      // or a natural exit racing it. It is not an outcome to show, and it must
      // not reach whatever sign-in the user starts next; it only tells the
      // Close that the PTY is gone.
      if (closing.has(e.id)) {
        closeWaiters.get(e.id)?.();
        return;
      }
      live.delete(e.id);
      spawned.delete(e.id);
      setExit(e.id, e);
    }),
  ]).then(
    () => undefined,
    (err) => {
      taps = undefined;
      throw err;
    },
  );
  return taps;
}

/** Per-provider open promise, so keystrokes and the first resize (which xterm
 *  fires as soon as it fits its host) wait for the PTY to exist instead of
 *  racing it and being dropped. */
const opening = new Map<string, Promise<void>>();

/** Per-provider close in flight. A close waits for the open it is closing, so a
 *  "Sign in" clicked right after Close would otherwise reach the backend first,
 *  attach to the PTY the close is about to kill, and then lose it. The next
 *  open waits here instead, so Close always means "this flow is gone" and a
 *  reopen always starts fresh. */
const closing = new Map<string, Promise<void>>();

/** Providers whose PTY we believe exists on the backend: added when an open
 *  lands, removed by its exit or by the Close that kills it. Tells a Close
 *  whether an exit event is still owed. */
const spawned = new Set<string>();

/** Per-provider resolver a Close parks to learn that its kill has landed (see
 *  the exit tap). */
const closeWaiters = new Map<string, () => void>();

/** How long a Close waits for the killed flow's exit event before giving up.
 *  The kill escalates HUP → TERM → KILL over well under a second; this is only
 *  a ceiling so a lost event can't wedge the button. */
const CLOSE_EXIT_GRACE_MS = 3000;

/** Run a provider's sign-in from the top: clears what the last run printed and
 *  its outcome, so the terminal starts blank. This is "Sign in" on a provider
 *  that hasn't been signed in during this session, and "Run again" after one
 *  finished. */
export function runLogin(id: string, cols: number, rows: number): Promise<void> {
  output.drop(id);
  setExit(id, undefined);
  started.add(id);
  live.add(id);
  const open = openPty(id, cols, rows);
  // Recorded synchronously, so keystrokes and the first resize queue behind the
  // *whole* open — tap registration included — and not just behind the spawn.
  opening.set(id, open);
  return open;
}

/** The open itself: taps first, PTY second, so nothing the flow emits can be
 *  missed. A failure of either step becomes the session's outcome — the row
 *  shows it and offers "Run again" rather than sitting in a running state — so
 *  the returned promise always resolves and callers can await it safely. */
async function openPty(id: string, cols: number, rows: number): Promise<void> {
  try {
    // A Close still tearing down the previous flow goes first (see `closing`).
    await closing.get(id);
    await ensureTaps();
    // Marked before the call, not after: a CLI that exits the instant it
    // spawns delivers its exit while the open is still resolving, and that
    // exit must find the mark to clear it — or a later Close would wait for an
    // exit that already came.
    spawned.add(id);
    await api.openProviderLogin(id, cols, rows);
  } catch (err) {
    // Nothing is attached, so report it through the same channel an exit uses.
    spawned.delete(id);
    live.delete(id);
    // Unless a Close landed while this was in flight: that session is gone and
    // must not be resurrected by a stale failure.
    if (started.has(id)) setExit(id, { id, success: false, message: String(err) });
  }
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
 *  "Sign in" then starts a fresh flow rather than re-attaching to this one.
 *
 *  Resolves once the flow is truly gone: the open it may still be waiting on
 *  has landed, the backend has killed the PTY, and the exit that kill produces
 *  has been received (or given up on after a grace period). Callers dismiss
 *  the terminal only after this, and the next open queues behind it, so no
 *  later sign-in can attach to, or inherit the outcome of, the flow closed here. */
export function closeLogin(id: string): Promise<void> {
  const open = opening.get(id);
  started.delete(id);
  live.delete(id);
  output.drop(id);
  setExit(id, undefined);
  opening.delete(id);
  // Parked before anything is awaited, so an exit that races the close is
  // still caught (the tap only signals a waiter while `closing` has the id).
  let exited!: () => void;
  const exit = new Promise<void>((resolve) => {
    exited = resolve;
  });
  closeWaiters.set(id, exited);
  const done = (async () => {
    // Let an in-flight open land first: a close that overtakes it would be a
    // no-op on the backend and leave the PTY it then spawns running unattended.
    await open;
    // Only a PTY that actually came up owes us an exit event.
    const expectExit = spawned.delete(id);
    await api.closeProviderLogin(id).catch((err) => {
      console.error("closeProviderLogin failed", err);
    });
    if (expectExit) {
      await Promise.race([exit, new Promise((r) => setTimeout(r, CLOSE_EXIT_GRACE_MS))]);
    }
  })();
  // Recorded synchronously so an open issued before this settles queues behind
  // it (see `openPty`). A later close for the same id replaces the entry, so
  // only the newest one is ever awaited.
  closing.set(id, done);
  return done.finally(() => {
    if (closing.get(id) === done) closing.delete(id);
    if (closeWaiters.get(id) === exited) closeWaiters.delete(id);
  });
}
