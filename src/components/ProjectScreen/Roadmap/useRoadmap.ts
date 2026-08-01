// The Roadmap's single source of truth: the board's items and the PM thread
// that edits them. The PM never writes to the board directly — it *proposes*,
// the board renders the proposal as ghost rows, and the user commits. That rule
// lives here, so both columns stay honest about what is real.
//
// The board is persisted, per project, in `roadmap_items` (src-tauri/src/roadmap).
// This hook loads it once for the current project and then keeps it live off the
// `roadmap:item` / `roadmap:item-deleted` events, upserting the full row by id —
// the same fetch-once-then-upsert shape `useRuns` uses. Every mutation goes
// through the API, including the ones the thread triggers: accepting a proposal
// creates real rows, and takes their real, backend-allocated codes.
//
// The PM thread itself is still the canned script in `mockData.ts` — a real
// agent replaces it next. What is no longer fake is everything it writes.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  type NewRoadmapItem,
  onRoadmapItem,
  onRoadmapItemDeleted,
  type RoadmapItem,
  type RoadmapItemPatch,
} from "@/api";
import type { UIAnswer } from "@/components/Workspace/messages/UserInput/parse";
import { useAppStore } from "@/store";
import { freeFormBeat, PRODUCT_MAP, SCRIPT, SEED_THREAD } from "./mockData";
import type { BoardItem, Horizon, PmBody, PmMessage, ProposalChange } from "./types";
import { toBoardItem } from "./types";

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

/** Placeholder key for a row the PM proposed mid-conversation. It is never
 *  displayed — a ghost renders as "NEW", because its real code isn't allocated
 *  until the user accepts — it only has to be unique so React keys and
 *  `openCodes` can tell two pending ghosts apart. */
let draftSeq = 0;
const nextDraftCode = () => `draft-${++draftSeq}`;

export type BoardTab = "roadmap" | "map";

/** A shipped item leaves the board entirely and survives only as the header's
 *  count, so "on the board" is every status but `done`. */
const isOnBoard = (i: RoadmapItem) => i.status !== "done";

export function useRoadmap(repoPath: string) {
  // The board is per project, not per repo: a multi-repo project has one
  // roadmap. Resolved from the pinned-repo list the sidebar already loads.
  const projectId =
    useAppStore((s) => s.workspace?.projects.find((p) => p.path === repoPath)?.project_id) ?? null;
  // Until the workspace itself is loaded, a missing project_id means "not known
  // yet", not "no project" — telling a populated board it's empty for a frame
  // would flash the empty state at someone who has a roadmap.
  const workspaceReady = useAppStore((s) => s.workspace != null);

  const [rows, setRows] = useState<RoadmapItem[]>([]);
  const [loading, setLoading] = useState(true);
  /** The last failure from a mutation with no form of its own to report into
   *  (a move, a delete, an accepted proposal). */
  const [error, setError] = useState<string | null>(null);
  const [messages, setMessages] = useState<PmMessage[]>(() => SEED_THREAD.map(withId));
  const [thinking, setThinking] = useState(false);
  /** Indices into SCRIPT already played — they drop out of the suggestions. */
  const [used, setUsed] = useState<ReadonlySet<number>>(() => new Set());
  const [tab, setTab] = useState<BoardTab>("roadmap");
  const [openCodes, setOpenCodes] = useState<ReadonlySet<string>>(() => new Set());
  const [focusCode, setFocusCode] = useState<string | null>(null);
  /** Codes highlighted because they just landed or just moved. */
  const [landed, setLanded] = useState<ReadonlySet<string>>(() => new Set());

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

  // ── the persisted board ────────────────────────────────────────────
  /** Upsert a row by id, appending new ones — the backend lists oldest-first
   *  and a new row is the newest, so append keeps the two in the same order. */
  const upsert = useCallback((row: RoadmapItem) => {
    setRows((prev) =>
      prev.some((r) => r.id === row.id)
        ? prev.map((r) => (r.id === row.id ? row : r))
        : [...prev, row],
    );
  }, []);

  useEffect(() => {
    if (!projectId) {
      setRows([]);
      setLoading(!workspaceReady);
      return;
    }
    let alive = true;
    setLoading(true);
    api
      .roadmapListItems(projectId)
      .then((items) => {
        if (!alive) return;
        setRows(items);
        setLoading(false);
      })
      .catch((e) => {
        if (!alive) return;
        setError(String(e));
        setLoading(false);
      });

    // Rows change from more than this screen (the PM agent's own writes, and
    // later the run queue), so the board follows the event rather than only
    // its own command results.
    const off = onRoadmapItem((row) => {
      if (row.project_id !== projectId) return;
      upsert(row);
    });
    const offDeleted = onRoadmapItemDeleted((id) => {
      setRows((prev) => prev.filter((r) => r.id !== id));
    });

    return () => {
      alive = false;
      void off.then((f) => f());
      void offDeleted.then((f) => f());
    };
  }, [projectId, upsert, workspaceReady]);

  /** Light up rows for a moment after they land, then clear only those — a
   *  later landing must not have its highlight cut short by an earlier timer. */
  const markLanded = useCallback(
    (codes: string[]) => {
      if (!codes.length) return;
      setLanded((prev) => new Set([...prev, ...codes]));
      after(LANDED_MS, () =>
        setLanded((prev) => {
          const next = new Set(prev);
          for (const c of codes) next.delete(c);
          return next;
        }),
      );
    },
    [after],
  );

  // ── derived ────────────────────────────────────────────────────────
  const items = useMemo(() => rows.filter(isOnBoard).map(toBoardItem), [rows]);
  /** Shipped items aren't on the board; the header carries the count. */
  const shipped = useMemo(() => rows.filter((r) => !isOnBoard(r)).length, [rows]);

  const pending = messages.find((m) => m.kind === "proposal" && !m.resolved) ?? null;
  const openQuestion = messages.find((m) => m.kind === "question" && !m.answer) ?? null;
  /** The user owes an answer or a decision before the thread can move on. */
  const blocked = Boolean(pending || openQuestion);
  /** A reply is still landing. Also closes the composer: a second message sent
   *  mid-reply would interleave its beats with the first and could leave two
   *  proposals open at once, only one of which the board renders. */
  const busy = thinking;

  const ghosts: BoardItem[] =
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

  // ── board mutations ────────────────────────────────────────────────
  /** Add a row; the code is allocated backend-side. Throws on failure, because
   *  its caller is a form that can say so inline. */
  const addItem = useCallback(
    async (input: NewRoadmapItem) => {
      if (!projectId) throw new Error("This repo isn't part of a project yet.");
      const row = await api.roadmapCreateItem(projectId, input);
      upsert(row);
      markLanded([row.code]);
      return row;
    },
    [projectId, upsert, markLanded],
  );

  /** Edit a row. Throws, like `addItem` — its caller is the same form. */
  const editItem = useCallback(
    async (id: string, patch: RoadmapItemPatch) => {
      const row = await api.roadmapUpdateItem(id, patch);
      upsert(row);
      return row;
    },
    [upsert],
  );

  /** Run a mutation whose caller has nowhere to render a failure, and surface
   *  it on the board instead of dropping it on the floor. */
  const guarded = useCallback(async (fn: () => Promise<unknown>) => {
    try {
      setError(null);
      await fn();
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const moveItem = useCallback(
    (id: string, to: Horizon) =>
      guarded(async () => {
        const row = await api.roadmapUpdateItem(id, { horizon: to });
        upsert(row);
        markLanded([row.code]);
      }),
    [guarded, markLanded, upsert],
  );

  const removeItem = useCallback(
    (id: string) =>
      guarded(async () => {
        await api.roadmapDeleteItem(id);
        setRows((prev) => prev.filter((r) => r.id !== id));
      }),
    [guarded],
  );

  const clearError = useCallback(() => setError(null), []);

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
      play(freeFormBeat(body, nextDraftCode()));
    },
    [blocked, busy, play, push, used],
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
  /** Turn a proposal into rows. An add becomes a real item — the ghost's code
   *  was only ever a placeholder, so what the user sees afterwards is the code
   *  the backend allocated. A move addresses its row by code; one naming a code
   *  this project doesn't have is skipped rather than failing the whole commit.
   *  Returns the codes that actually landed. */
  const applyChanges = useCallback(
    async (changes: ProposalChange[]) => {
      if (!projectId) throw new Error("This repo isn't part of a project yet.");
      const codes: string[] = [];
      for (const c of changes) {
        if (c.kind === "add") {
          const row = await api.roadmapCreateItem(projectId, {
            title: c.item.title,
            why: c.item.why,
            horizon: c.item.horizon,
            size: c.item.size ?? null,
            area: c.item.area ?? null,
            source: "pm",
            epic: c.item.epic ?? null,
            accept: c.item.accept ?? [],
            deps: c.item.deps ?? [],
          });
          upsert(row);
          codes.push(row.code);
        } else {
          const target = rows.find((r) => r.code === c.code);
          if (!target) continue;
          const row = await api.roadmapUpdateItem(target.id, { horizon: c.to });
          upsert(row);
          codes.push(row.code);
        }
      }
      markLanded(codes);
      return codes;
    },
    [markLanded, projectId, rows, upsert],
  );

  const accept = useCallback(
    async (id: string) => {
      const p = messages.find((m) => m.id === id);
      if (p?.kind !== "proposal" || p.resolved) return;
      let codes: string[];
      try {
        setError(null);
        codes = await applyChanges(p.changes);
      } catch (e) {
        // The proposal stays open: nothing was committed, so the user can
        // retry once whatever failed is fixed.
        setError(String(e));
        return;
      }
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
    shipped,
    loading,
    /** No project row for this repo — the board can be read but not written,
     *  so the write affordances stay out of the way. */
    readOnly: workspaceReady && projectId == null,
    error,
    clearError,
    map: PRODUCT_MAP,
    tab,
    setTab,
    openCodes,
    toggleItem,
    focusCode,
    focusItem,
    landed,
    addItem,
    editItem,
    moveItem,
    removeItem,
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
