// A cache of live xterm `Terminal`s, one per cached surface (today: each
// agent's native TUI), kept alive while its React view is unmounted.
//
// The native view used to dispose its Terminal on every agent switch and
// rebuild the screen by replaying the 256 KB ring buffer in ./buffers. That
// slab is cut at an arbitrary byte offset, so the replay begins mid ANSI escape
// sequence, mid UTF-8 codepoint and mid TUI frame — and a TUI's redraws blow
// past 256 KB in seconds, so the cut was the rule rather than the exception:
// re-selecting an agent painted garbage until the TUI happened to repaint. The
// rebuilt Terminal also started at xterm's default 80x24 while the PTY was
// still at its last fitted size, so the replay reflowed against the wrong
// width before FitAddon caught up a frame later.
//
// Re-parenting a still-live Terminal instead means there is nothing to replay
// and nothing to corrupt: the screen is already correct and already the right
// size. xterm cooperates — its renderer pauses on an IntersectionObserver while
// the element is out of the tree and does a full refresh when it comes back.

import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { type ITerminalOptions, Terminal } from "@xterm/xterm";
import { setTerminalCacheHooks } from "./buffers";

export interface LiveTerminal {
  term: Terminal;
  fit: FitAddon;
  /** The element `term` was `open()`ed into, exactly once. xterm offers no way
   *  to re-open a Terminal into a different element without rebuilding its DOM,
   *  so mounting moves THIS node into the visible slot and unmounting detaches
   *  it again. */
  host: HTMLDivElement;
  /** Absent when the WebGL context couldn't be created (DOM renderer in use). */
  webgl?: WebglAddon;
  /** Teardown for the owner's terminal-lifetime wiring (output sink, PTY
   *  listeners). Runs on disposal only, never on unmount — that is what keeps a
   *  cached terminal's sink registered so a background agent's screen stays
   *  current and needs no replay when it is selected again. */
  teardown?: () => void;
}

const cache = new Map<string, LiveTerminal>();

/** Cache key for an agent's native TUI terminal. Namespaced so another cached
 *  surface for the same agent (e.g. its side shell) can never collide with it. */
export const nativeTerminalKey = (agentId: string) => `native:${agentId}`;

/** Build a Terminal + FitAddon (+ WebGL renderer when available) inside a host
 *  element appended to `parent`.
 *
 *  The host is created here rather than opening straight into `parent` because
 *  a cached terminal outlives the React element it was first mounted in: the
 *  one and only `open()` has to target a node we own and can move. It is
 *  appended BEFORE `open()` — xterm measures cell size at open time and a
 *  detached element measures zero. */
export function createTerminal(parent: HTMLElement, options: ITerminalOptions): LiveTerminal {
  const host = document.createElement("div");
  // Fill the slot exactly. FitAddon derives cols/rows from the terminal
  // element's parent, so a host that didn't fill would mis-size the PTY.
  host.style.cssText = "position:absolute;inset:0";
  parent.appendChild(host);

  const term = new Terminal(options);
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(host);

  // GPU renderer: positions cells on exact device pixels. The default DOM
  // renderer rasterizes the last column short on fractional cell widths
  // (e.g. 7.42px @ dpr 2), clipping the rightmost glyph. Fall back to the
  // DOM renderer if the WebGL context is unavailable or gets lost.
  let webgl: WebglAddon | undefined;
  try {
    webgl = new WebglAddon();
    webgl.onContextLoss(() => webgl?.dispose());
    term.loadAddon(webgl);
  } catch {
    // WebGL unavailable — DOM renderer remains in use
  }

  return { term, fit, host, webgl };
}

/** Tear a terminal down for good, owner wiring included. */
export function disposeTerminal(entry: LiveTerminal) {
  entry.host.remove();
  entry.teardown?.();
  // Dispose the WebGL renderer BEFORE the terminal. Tearing it down after the
  // core is gone dereferences a disposed _core._store and throws (React
  // StrictMode's dev mount→unmount cycle triggers this every time). Guarded so
  // the terminal's own addon disposal can't double-free it.
  try {
    entry.webgl?.dispose();
  } catch {
    /* already disposed */
  }
  entry.term.dispose();
}

/** Get the terminal cached under `key`, re-parented into `parent`, or create
 *  and cache a new one there. The re-attach path is the point of this module:
 *  no rebuild, no replay, same pixels.
 *
 *  `created` tells the caller whether the one-time wiring still has to run —
 *  false on a re-attach, and false on the second half of StrictMode's dev
 *  mount→unmount→mount, which is what keeps that cycle from registering
 *  duplicate listeners. */
export function acquireTerminal(
  key: string,
  parent: HTMLElement,
  options: ITerminalOptions,
): { entry: LiveTerminal; created: boolean } {
  const existing = cache.get(key);
  if (existing) {
    parent.appendChild(existing.host);
    return { entry: existing, created: false };
  }
  const entry = createTerminal(parent, options);
  cache.set(key, entry);
  return { entry, created: true };
}

/** Wipe a cached native terminal's screen in place, keeping it cached and its
 *  output sink registered. Called when the agent's PTY restarts (view switch,
 *  resume) and its replay history is dropped: the live terminal is showing that
 *  very history, so leaving it would re-attach a dead session's frame on the
 *  next mount. `reset()` clears buffer, scrollback and modes without touching
 *  the DOM or the fitted geometry. */
function resetAgentTerminal(agentId: string) {
  cache.get(nativeTerminalKey(agentId))?.term.reset();
}

/** Dispose an agent's cached native terminal — the agent is gone. */
function evictAgentTerminal(agentId: string) {
  const key = nativeTerminalKey(agentId);
  const entry = cache.get(key);
  if (!entry) return;
  cache.delete(key);
  disposeTerminal(entry);
}

// Hand the cache's lifecycle to ./buffers rather than have the store call in
// here: importing xterm outside a browser throws (`self is not defined`), and
// the store actions that clear an agent's PTY state are exercised in node-env
// tests. Registering keeps xterm out of that import graph entirely.
setTerminalCacheHooks({ reset: resetAgentTerminal, evict: evictAgentTerminal });
