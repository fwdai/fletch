// Building and tearing down an xterm `Terminal` with the app's renderer setup.
// The lifecycle around it (fit, theme, focus) is `useXterm`'s.

import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { type ITerminalOptions, Terminal } from "@xterm/xterm";

export interface LiveTerminal {
  term: Terminal;
  fit: FitAddon;
  /** The element `term` was `open()`ed into. */
  host: HTMLDivElement;
  /** Absent when the WebGL context couldn't be created (DOM renderer in use). */
  webgl?: WebglAddon;
  /** Teardown for the owner's wiring (output sink, PTY listeners), run when
   *  the terminal is disposed. */
  teardown?: () => void;
}

/** Build a Terminal + FitAddon (+ WebGL renderer when available) inside a host
 *  element appended to `parent`.
 *
 *  The host is appended BEFORE `open()` — xterm measures cell size at open
 *  time and a detached element measures zero. */
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
