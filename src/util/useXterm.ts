import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { type ITerminalOptions, Terminal } from "@xterm/xterm";
import { type DependencyList, useEffect, useRef } from "react";
import "@xterm/xterm/css/xterm.css";

/** Options shared by every terminal in the app; callers override per use. */
const XTERM_BASE_OPTIONS: ITerminalOptions = {
  fontFamily: "ui-monospace, 'SF Mono', Menlo, monospace",
  cursorBlink: true,
  cursorStyle: "block",
  allowProposedApi: false,
  macOptionIsMeta: true,
};

/** Mount an xterm `Terminal` + `FitAddon` into a host element and own the
 *  fit / resize / focus / dispose lifecycle.
 *
 *  The returned ref must be attached to a `.xterm-host` element (an absolute
 *  fill of its flex slot, inset via offsets — which FitAddon reads correctly,
 *  unlike padding). `options` is merged over the shared base.
 *
 *  Callers wire their own data flow in `onReady`: load extra addons, replay
 *  buffered output, hook `onData`/`onResize`, register a sink, etc., and
 *  return a cleanup that the hook runs on unmount, before disposing the
 *  terminal. The whole lifecycle re-runs whenever `deps` change (e.g. a
 *  different agent id).
 *
 *  `hostOptions.autoFocus` (default true) focuses the terminal after mount.
 *  Read-only surfaces (e.g. a log view) pass false so mounting doesn't pull
 *  keyboard focus away from the editor.
 */
export function useXterm(
  options: ITerminalOptions,
  onReady: (term: Terminal) => (() => void) | undefined,
  deps: DependencyList,
  hostOptions?: { autoFocus?: boolean },
) {
  const autoFocus = hostOptions?.autoFocus ?? true;
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const term = new Terminal({ ...XTERM_BASE_OPTIONS, ...options });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);

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

    const cleanup = onReady(term);

    const initialFit = requestAnimationFrame(() => {
      try {
        fit.fit();
      } catch {
        /* not measurable yet */
      }
    });

    // Debounce refits to when the panel stops resizing. Fitting on every
    // ResizeObserver tick makes the WebGL renderer clear and redraw its canvas
    // each frame, which flashes during a drag. The terminal holds its size
    // mid-drag (briefly clipped by the host's overflow) and reflows once.
    let resizeTimer: ReturnType<typeof setTimeout> | undefined;
    const ro = new ResizeObserver(() => {
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        try {
          fit.fit();
        } catch {
          /* container may be hidden */
        }
      }, 100);
    });
    ro.observe(el);
    if (autoFocus) term.focus();

    return () => {
      cancelAnimationFrame(initialFit);
      if (resizeTimer) clearTimeout(resizeTimer);
      ro.disconnect();
      cleanup?.();
      // Dispose the WebGL renderer BEFORE the terminal. Tearing it down after
      // the core is gone dereferences a disposed _core._store and throws
      // (React StrictMode's dev mount→unmount cycle triggers this every time).
      // Guarded so the terminal's own addon disposal can't double-free it.
      try {
        webgl?.dispose();
      } catch {
        /* already disposed */
      }
      term.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
    // biome-ignore lint/correctness/useExhaustiveDependencies: caller supplies the dep list
  }, deps);

  return containerRef;
}
