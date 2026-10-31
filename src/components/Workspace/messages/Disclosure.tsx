// The motion and reveal plumbing every collapsible transcript row shares: a
// body that grows open and shrinks shut, a label that crossfades when what it
// says changes, a live label that doesn't flicker, and the three ways a folded
// row opens without a click of its own — ⌥-click on an ancestor, a find query
// that matches inside it, a reveal request for a row it holds.

import {
  createContext,
  type ReactNode,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { useAppStore } from "@/store";
import type { ViewItem } from "./pair";
import { REVEAL_MIN_QUERY, searchText } from "./steps";

/** True below a disclosure opened with ⌥: everything under it opens too, the
 *  way ⌥-click on a disclosure triangle does in Finder. */
export const DeepOpen = createContext(false);

/** The open find-in-conversation query, lower-cased ("" when closed). Folded
 *  rows holding a match open while it stands, so the find bar — which walks
 *  the rendered text — can see and highlight it. */
export const RevealQuery = createContext("");

/** Whether the find query, if any, matches inside `items`. */
export function useRevealed(items: ViewItem[]): boolean {
  const query = useContext(RevealQuery);
  if (query.length < REVEAL_MIN_QUERY) return false;
  return items.some((it) => searchText(it).includes(query));
}

/** Like `useRevealed`, for one row's own text. */
export function useRevealedText(text: string | undefined): boolean {
  const query = useContext(RevealQuery);
  return query.length >= REVEAL_MIN_QUERY && !!text?.includes(query);
}

/** Open state for a disclosure: starts open under a ⌥-opened ancestor, and
 *  `toggle` reads ⌥ off the click to open (or close) everything below.
 *  `reveal` opens it without motion, for a jump that has to land on a row
 *  inside at its final place. */
export function useDisclosure(): {
  open: boolean;
  deep: boolean;
  instant: boolean;
  setOpen: (open: boolean) => void;
  reveal: () => void;
  toggle: (e: { altKey: boolean }) => void;
} {
  const parentDeep = useContext(DeepOpen);
  const [open, setOpen] = useState(parentDeep);
  const [deep, setDeep] = useState(parentDeep);
  const [instant, setInstant] = useState(false);
  return {
    open,
    deep: deep || parentDeep,
    instant,
    setOpen,
    reveal: () => {
      setInstant(true);
      setOpen(true);
    },
    toggle: (e) => {
      setInstant(false);
      setDeep(e.altKey && !open);
      setOpen(!open);
    },
  };
}

/** Open when a reveal request (store `chatFocus`) names a tool call among
 *  `items` — the sub-agent card the user just came back from, folded inside
 *  an older turn. Sticky: the request is consumed by the row it names, which
 *  only mounts once this has opened. Selects the request itself (null almost
 *  always) and looks through `items` only when it changes — never on the
 *  store updates every streaming token makes. */
export function useFocusInside(
  agentId: string | undefined,
  items: ViewItem[],
  open: () => void,
): void {
  const focus = useAppStore((s) => s.chatFocus);
  const inside =
    focus !== null &&
    focus.agentId === agentId &&
    items.some((it) => it.kind === "tool_pair" && it.call.id === focus.toolUseId);
  const openRef = useRef(open);
  openRef.current = open;
  useEffect(() => {
    if (inside) openRef.current();
  }, [inside]);
}

const REDUCED_MOTION =
  typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-reduced-motion: reduce)")
    : null;

/** Exit animations end by unmounting; if one never fires its `animationend`
 *  (a hidden tab, reduced motion) this is how long the body waits anyway. */
const EXIT_MS = 260;

/** A disclosure's body: grows from nothing on open and shrinks back on close,
 *  then unmounts — closed bodies cost the transcript nothing. Mounts in the
 *  same render that opens it, so a find query that opened it sees its text at
 *  once. `instant` skips the motion: a body a find query opens has to sit at
 *  its full height when the find bar measures where to scroll. */
export function DisclosureBody({
  open,
  instant,
  className,
  children,
}: {
  open: boolean;
  instant?: boolean;
  className?: string;
  children: ReactNode;
}) {
  const [phase, setPhase] = useState<"entering" | "open" | "closing" | "closed">(
    open ? "open" : "closed",
  );
  const was = useRef(open);

  // Layout effect: the phase flips before paint, so an opening body never
  // shows one frame at full height first.
  useLayoutEffect(() => {
    if (open === was.current) return;
    was.current = open;
    if (instant || REDUCED_MOTION?.matches) {
      setPhase(open ? "open" : "closed");
      return;
    }
    setPhase(open ? "entering" : "closing");
  }, [open, instant]);

  useEffect(() => {
    if (phase !== "closing" && phase !== "entering") return;
    const t = window.setTimeout(() => setPhase(phase === "closing" ? "closed" : "open"), EXIT_MS);
    return () => clearTimeout(t);
  }, [phase]);

  // Before the layout effect has run, an opening body renders as entering.
  const shown = open && phase === "closed" ? (instant ? "open" : "entering") : phase;
  if (shown === "closed" && !open) return null;
  return (
    <div
      className={`disclosure is-${shown}${className ? ` ${className}` : ""}`}
      onAnimationEnd={(e) => {
        if (e.target !== e.currentTarget) return;
        setPhase((p) => (p === "closing" ? "closed" : p === "entering" ? "open" : p));
      }}
    >
      <div className="disclosure-inner">{children}</div>
    </div>
  );
}

/** Crossfades between what it shows whenever `k` changes: the old content
 *  lifts away as the new rises in. Same `k`, same element — content updating
 *  in place (a count ticking) doesn't animate. */
export function Ticker({
  k,
  className,
  children,
}: {
  k: string;
  className?: string;
  children: ReactNode;
}) {
  const last = useRef<{ k: string; node: ReactNode }>({ k, node: children });
  const [shownKey, setShownKey] = useState(k);
  const [leaving, setLeaving] = useState<{ k: string; node: ReactNode } | null>(null);
  if (shownKey !== k) {
    // Derived state: the outgoing copy is what the old key last rendered.
    setShownKey(k);
    setLeaving(REDUCED_MOTION?.matches ? null : last.current);
  }
  useEffect(() => {
    last.current = { k, node: children };
  });
  return (
    <span className={`ticker${className ? ` ${className}` : ""}`}>
      {leaving && (
        <span
          key={`out:${leaving.k}`}
          className="ticker-item is-out"
          aria-hidden
          onAnimationEnd={() => setLeaving(null)}
        >
          {leaving.node}
        </span>
      )}
      <span key={`in:${k}`} className={leaving ? "ticker-item is-in" : "ticker-item"}>
        {children}
      </span>
    </span>
  );
}

/** A live value that changes no faster than it can be read: each one holds
 *  for `dwellMs` before the next replaces it, and an `idle` one (nothing in
 *  flight between two calls) waits `idleMs` first, so "Thinking" only shows
 *  when the agent really is. `key` says when the value has changed; the
 *  value travels with it, so a label and its icon switch together. */
export function useSteady<T>(
  value: T,
  key: string,
  idle: boolean,
  dwellMs = 700,
  idleMs = 1400,
): T {
  const [shown, setShown] = useState({ key, value });
  const since = useRef(Date.now());
  const latest = useRef(value);
  latest.current = value;
  useEffect(() => {
    if (key === shown.key) return;
    const held = Date.now() - since.current;
    const wait = Math.max(dwellMs - held, idle ? idleMs : 0, 0);
    const t = window.setTimeout(() => {
      since.current = Date.now();
      setShown({ key, value: latest.current });
    }, wait);
    return () => clearTimeout(t);
  }, [key, idle, shown.key, dwellMs, idleMs]);
  return shown.value;
}
