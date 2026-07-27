import type { ITerminalOptions, Terminal } from "@xterm/xterm";
import { type DependencyList, useEffect, useRef } from "react";
import { acquireTerminal, createTerminal, disposeTerminal } from "@/pty/terminals";
import { resolveTheme } from "./xtermTheme";
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
 *  return a cleanup that the hook runs before disposing the terminal. The whole
 *  lifecycle re-runs whenever `deps` change (e.g. a different agent id).
 *
 *  `hostOptions.autoFocus` (default true) focuses the terminal after mount.
 *  Read-only surfaces (e.g. a log view) pass false so mounting doesn't pull
 *  keyboard focus away from the editor.
 *
 *  Theming is owned here: every terminal in the app renders the current
 *  palette (`resolveTheme`) and re-resolves it live when the theme or accent
 *  changes, so no caller has to wire its own observer. A caller that passes
 *  `options.theme` pins its own palette and opts out of that reactivity.
 *
 *  `hostOptions.cacheKey` opts into the live-terminal cache (src/pty/terminals):
 *  the `Terminal` outlives this component and is re-attached on the next mount
 *  under the same key instead of being rebuilt. `onReady` then runs ONCE per
 *  terminal rather than once per mount, and its cleanup runs at eviction — so
 *  sinks and PTY listeners stay live in the background and nothing has to be
 *  replayed on return. The key must be derived from the same values as `deps`,
 *  or a dep change would re-attach the wrong terminal.
 *
 *  `hostOptions.onMount` is the per-MOUNT counterpart to `onReady`: it runs on
 *  every mount, including a re-attach, and its cleanup runs on every unmount.
 *  Anything holding this component instance's state belongs here rather than in
 *  `onReady`, whose closure would otherwise be pinned to the first mount.
 */
export function useXterm(
  options: ITerminalOptions,
  onReady: (term: Terminal) => (() => void) | undefined,
  deps: DependencyList,
  hostOptions?: {
    autoFocus?: boolean;
    cacheKey?: string;
    /** Per-mount wiring; see the note on `cacheKey` above. */
    onMount?: (term: Terminal) => (() => void) | undefined;
  },
) {
  const autoFocus = hostOptions?.autoFocus ?? true;
  const cacheKey = hostOptions?.cacheKey;
  const onMount = hostOptions?.onMount;
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    // The app palette sits between the shared base and the caller's overrides,
    // so passing `theme` still wins (and pins it — see the observer below).
    const merged = { ...XTERM_BASE_OPTIONS, theme: resolveTheme(), ...options };
    const { entry, created } =
      cacheKey === undefined
        ? { entry: createTerminal(el, merged), created: true }
        : acquireTerminal(cacheKey, el, merged);
    const { term, fit } = entry;

    // A cached terminal was built with whatever palette was current then, and
    // its observer was disconnected while unmounted — so re-resolve on every
    // mount or a theme flip that happened in the background would persist.
    if (options.theme === undefined && !created) term.options.theme = resolveTheme();

    // Re-resolve the palette when the app flips dark↔light (a class swap on
    // <html>) or the accent changes (CSS vars set inline on the same element).
    // Skipped when the caller pinned its own theme.
    let themeObserver: MutationObserver | undefined;
    if (options.theme === undefined) {
      themeObserver = new MutationObserver(() => {
        term.options.theme = resolveTheme();
      });
      themeObserver.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["class", "style"],
      });
    }

    // One-time wiring, keyed to the terminal rather than to this mount. A
    // re-attach skips it: the listeners and output sink from the first mount
    // are still attached — which is also what keeps StrictMode's dev
    // mount→unmount→mount from registering a duplicate set.
    if (created) entry.teardown = onReady(term);
    const unmount = onMount?.(term);

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
      themeObserver?.disconnect();
      unmount?.();
      if (cacheKey === undefined) {
        disposeTerminal(entry);
        return;
      }
      // Cached: pull the host out of the tree but leave the terminal running.
      // xterm's renderer pauses itself while detached and full-refreshes on
      // re-attach, so the screen survives intact. Removing an already-removed
      // node is a no-op, which covers an eviction that landed between this
      // mount and this unmount.
      entry.host.remove();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
    // biome-ignore lint/correctness/useExhaustiveDependencies: caller supplies the dep list
  }, deps);

  return containerRef;
}
