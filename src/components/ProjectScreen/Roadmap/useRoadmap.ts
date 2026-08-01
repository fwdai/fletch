// The Roadmap's single source of truth: the board's items and the PM thread
// that edits them. The PM never writes to the board directly — it *proposes*,
// the board renders the proposal as ghost rows, and the user commits. That rule
// lives here, so both columns stay honest about what is real.
//
// State is in-memory only (see `mockData.ts`); reloading the page resets it.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { UIAnswer } from "@/components/Workspace/messages/UserInput/parse";
import {
  CODE_PREFIX,
  FIRST_FREE_CODE,
  freeFormBeat,
  PRODUCT_MAP,
  SCRIPT,
  SEED_ITEMS,
  SEED_THREAD,
  SHIPPED_COUNT,
} from "./mockData";
import type { Horizon, PmBody, PmMessage, ProposalChange, RoadmapItem } from "./types";

/** Beat pacing, in ms after the previous message. A probe reads as work, so it
 *  lands slower than a line of prose. */
const BEAT_DELAY: Record<PmBody["kind"], number> = {
  thinking: 420,
  probe: 1150,
  question: 900,
  user: 780,
  text: 780,
  proposal: 780,
  landed: 780,
};

/** How long a row stays highlighted after landing on the board. */
const LANDED_MS = 2200;
/** How long a focused row keeps its ring after being jumped to. */
const FOCUS_MS = 2200;

let seq = 0;
const nextId = () => `pm-${++seq}`;
const withId = (body: PmBody): PmMessage => ({ ...body, id: nextId() });

export type BoardTab = "roadmap" | "map";

export function useRoadmap() {
  const [items, setItems] = useState<RoadmapItem[]>(SEED_ITEMS);
  const [messages, setMessages] = useState<PmMessage[]>(() => SEED_THREAD.map(withId));
  const [thinking, setThinking] = useState(false);
  /** Indices into SCRIPT already played — they drop out of the suggestions. */
  const [used, setUsed] = useState<ReadonlySet<number>>(() => new Set());
  /** Free-form ideas captured so far, so minted codes don't collide. */
  const [captured, setCaptured] = useState(0);
  const [tab, setTab] = useState<BoardTab>("roadmap");
  const [openCodes, setOpenCodes] = useState<ReadonlySet<string>>(() => new Set());
  const [focusCode, setFocusCode] = useState<string | null>(null);

  // Every pending beat, so unmounting mid-conversation can't set state on a
  // dead component.
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);
  useEffect(() => {
    const pending = timers.current;
    return () => {
      for (const t of pending) clearTimeout(t);
    };
  }, []);
  const after = useCallback((ms: number, fn: () => void) => {
    timers.current.push(setTimeout(fn, ms));
  }, []);

  const push = useCallback((body: PmBody) => setMessages((m) => [...m, withId(body)]), []);

  // ── derived ────────────────────────────────────────────────────────
  const pending = messages.find((m) => m.kind === "proposal" && !m.resolved) ?? null;
  const openQuestion = messages.find((m) => m.kind === "question" && !m.answer) ?? null;
  /** The user owes an answer or a decision before the thread can move on. */
  const blocked = Boolean(pending || openQuestion);
  /** A reply is still landing. Also closes the composer: a second message sent
   *  mid-reply would interleave its beats with the first and could leave two
   *  proposals open at once, only one of which the board renders. */
  const busy = thinking;

  const ghosts =
    pending?.kind === "proposal"
      ? pending.changes.flatMap((c) => (c.kind === "add" ? [c.item] : []))
      : [];
  const moves =
    pending?.kind === "proposal"
      ? pending.changes.flatMap((c) => (c.kind === "move" ? [c] : []))
      : [];

  const counts = useMemo(() => {
    const by: Record<Horizon, number> = { now: 0, next: 0, later: 0 };
    for (const i of items) by[i.horizon] += 1;
    return by;
  }, [items]);

  const suggestions = SCRIPT.filter((_, i) => !used.has(i))
    .slice(0, 2)
    .map((s) => s.prompt);

  // ── playing a beat ─────────────────────────────────────────────────
  const play = useCallback(
    (list: PmBody[]) => {
      setThinking(true);
      let at = 260;
      list.forEach((body, i) => {
        at += BEAT_DELAY[body.kind];
        after(at, () => {
          push(body);
          if (i === list.length - 1) setThinking(false);
        });
      });
    },
    [after, push],
  );

  const send = useCallback(
    (text: string) => {
      const body = text.trim();
      if (!body || blocked || busy) return;
      push({ kind: "user", body });

      // Match on what was actually said — suggestion chips can be clicked in
      // any order, and a beat only ever plays once.
      const idx = SCRIPT.findIndex(
        (s, i) => !used.has(i) && s.prompt.toLowerCase() === body.toLowerCase(),
      );
      if (idx >= 0) {
        setUsed((s) => new Set(s).add(idx));
        play(SCRIPT[idx].msgs);
        return;
      }
      setCaptured((n) => n + 1);
      play(freeFormBeat(body, `${CODE_PREFIX}${FIRST_FREE_CODE + captured}`));
    },
    [blocked, busy, captured, play, push, used],
  );

  const answerQuestion = useCallback(
    (id: string, answer: UIAnswer) => {
      const q = messages.find((m) => m.id === id);
      if (q?.kind !== "question") return;
      setMessages((arr) => arr.map((m) => (m.id === id ? { ...m, answer } : m)));
      play([
        { kind: "text", body: q.answered.text },
        { kind: "proposal", note: q.answered.note, changes: q.answered.changes },
      ]);
    },
    [messages, play],
  );

  // ── committing a proposal ──────────────────────────────────────────
  const applyChanges = useCallback(
    (changes: ProposalChange[]) => {
      const adds = changes.flatMap((c) =>
        c.kind === "add" ? [{ ...c.item, justAdded: true }] : [],
      );
      const moved = new Map(
        changes.flatMap((c) => (c.kind === "move" ? [[c.code, c.to] as const] : [])),
      );
      setItems((arr) => [
        ...arr.map((it) => {
          const to = moved.get(it.code);
          return to ? { ...it, horizon: to, justAdded: true } : it;
        }),
        ...adds,
      ]);
      // Clear only the rows this call lit up, so a later landing doesn't have
      // its highlight cut short by an earlier one's timer.
      const landed = new Set([...adds.map((a) => a.code), ...moved.keys()]);
      after(LANDED_MS, () =>
        setItems((arr) =>
          arr.map((it) => (landed.has(it.code) ? { ...it, justAdded: false } : it)),
        ),
      );
      return [...landed];
    },
    [after],
  );

  const accept = useCallback(
    (id: string) => {
      const p = messages.find((m) => m.id === id);
      if (p?.kind !== "proposal" || p.resolved) return;
      const codes = applyChanges(p.changes);
      setMessages((arr) => arr.map((m) => (m.id === id ? { ...m, resolved: "accepted" } : m)));
      after(420, () => push({ kind: "landed", codes }));
    },
    [after, applyChanges, messages, push],
  );

  const discard = useCallback(
    (id: string) => {
      setMessages((arr) => arr.map((m) => (m.id === id ? { ...m, resolved: "discarded" } : m)));
      after(360, () =>
        push({
          kind: "text",
          body: "Dropped — the board is unchanged. Tell me what's off about it and I'll re-shape.",
        }),
      );
    },
    [after, push],
  );

  // ── board interaction ──────────────────────────────────────────────
  const toggleItem = useCallback((code: string) => {
    setOpenCodes((s) => {
      const next = new Set(s);
      if (!next.delete(code)) next.add(code);
      return next;
    });
  }, []);

  /** Jump the board to an item and flash it — used by the "on the board" links. */
  const focusItem = useCallback(
    (code: string) => {
      setTab("roadmap");
      setFocusCode(code);
      setOpenCodes((s) => new Set(s).add(code));
      after(FOCUS_MS, () => setFocusCode(null));
    },
    [after],
  );

  return {
    // board
    items,
    ghosts,
    moves,
    counts,
    shipped: SHIPPED_COUNT,
    map: PRODUCT_MAP,
    tab,
    setTab,
    openCodes,
    toggleItem,
    focusCode,
    focusItem,
    // thread
    messages,
    busy,
    blocked,
    suggestions,
    send,
    answerQuestion,
    accept,
    discard,
  };
}

export type RoadmapState = ReturnType<typeof useRoadmap>;
