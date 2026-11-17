import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { AgentRecord } from "@/api";
import { HostChip } from "@/components/HostChip";
import { Icon } from "@/components/Icon";
import { Loader } from "@/components/ui/Loader";
import { ModalSheet } from "@/components/ui/Modal";
import { hostStore, onHost, useAppStore, useEveryHost } from "@/store";
import { entryOf } from "@/store/capabilities";
import { EngineScope } from "@/store/EngineScope";
import type { EnvironmentId } from "@/store/environments";
import { gateReason } from "@/store/gates";
import { basename, firstLine } from "@/util/format";

/** An archived workspace, and the host it was archived on. */
interface Archived {
  hostId: EnvironmentId;
  agent: AgentRecord;
}

/** Ids repeat across hosts, so a row is named by both. */
const rowKey = (item: Archived) => `${item.hostId}:${item.agent.id}`;

/** Every host's archived workspaces, newest first, with a paired host's
 *  named on its row. Restoring one brings it back on its own host and puts
 *  that host on screen with it selected. */
export function History() {
  const toggleHistory = useAppStore((s) => s.toggleHistory);
  const hostIds = useEveryHost((_s, hostId) => hostId);
  const agents = useEveryHost((s) => s.workspace?.agents);
  // `restore_agent` is on the wire, so this is null everywhere except against a
  // host from before the op — where the list stays browsable and only the
  // gesture is off, with the reason in the footer.
  const restoreGates = useEveryHost((s, hostId) => gateReason(entryOf(s, hostId), "restore"));
  const [query, setQuery] = useState("");
  const [restoringId, setRestoringId] = useState<string | null>(null);
  const [focusedIndex, setFocusedIndex] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const sheetRef = useRef<HTMLDivElement>(null);
  const close = useCallback(() => toggleHistory(false), [toggleHistory]);
  const gateOf = useCallback(
    (item: Archived) => restoreGates[hostIds.indexOf(item.hostId)] ?? null,
    [hostIds, restoreGates],
  );

  // Mutable refs so document-level handler always sees latest values
  const filteredRef = useRef<Archived[]>([]);
  const focusedIndexRef = useRef(0);
  const restoringIdRef = useRef<string | null>(null);

  const archived = useMemo(() => {
    const list = hostIds.flatMap((hostId, i) =>
      (agents[i] ?? []).filter((a) => a.archive).map((agent) => ({ hostId, agent })),
    );
    list.sort((a, b) => {
      const ta = a.agent.archive?.archived_at ?? "";
      const tb = b.agent.archive?.archived_at ?? "";
      return tb.localeCompare(ta);
    });
    return list;
  }, [hostIds, agents]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return archived;
    return archived.filter(({ agent: a }) => {
      if (a.name.toLowerCase().includes(q)) return true;
      if (a.task.toLowerCase().includes(q)) return true;
      const repo = a.archive?.repos[0]?.repo_path;
      if (repo && basename(repo).toLowerCase().includes(q)) return true;
      const branch = a.archive?.repos[0]?.branch_name;
      if (branch?.toLowerCase().includes(q)) return true;
      return false;
    });
  }, [archived, query]);

  const groups = useMemo(() => groupByDate(filtered), [filtered]);

  // Keep refs in sync every render
  filteredRef.current = filtered;
  focusedIndexRef.current = focusedIndex;
  restoringIdRef.current = restoringId;
  // The footer speaks for the row the cursor is on.
  const restoreGate = filtered[focusedIndex] ? gateOf(filtered[focusedIndex]) : null;

  // The restore answers its own failures with the error banner, and one that
  // went through has selected the workspace on its host, which is then put
  // on screen: History closes on whichever host it was opened over.
  const restore = useCallback(
    async (item: Archived) => {
      const host = hostStore(item.hostId);
      await host.getState().restore(item.agent.id);
      if (host.getState().selectedAgentId !== item.agent.id) return;
      close();
      onHost(item.hostId);
    },
    [close],
  );
  const restoreRef = useRef({ restore, gateOf });
  restoreRef.current = { restore, gateOf };

  // Reset cursor when filter results change
  useEffect(() => {
    setFocusedIndex(0);
  }, [filtered]);

  // Scroll focused row into view
  useEffect(() => {
    if (!listRef.current || filtered.length === 0) return;
    const rows = listRef.current.querySelectorAll<HTMLElement>(".hrow");
    rows[focusedIndex]?.scrollIntoView({ block: "nearest" });
  }, [focusedIndex, filtered]);

  // Keyboard: ↑↓ navigation, Enter to restore. Escape is `ModalSheet`'s.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const f = filteredRef.current;
      const idx = focusedIndexRef.current;
      const restoring = restoringIdRef.current;

      if (e.key === "ArrowDown") {
        e.preventDefault();
        setFocusedIndex(Math.min(idx + 1, f.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setFocusedIndex(Math.max(idx - 1, 0));
      } else if (e.key === "Enter") {
        // Only restore for Enter aimed at History itself. The same Enter
        // keystroke that selects `/resume` from the composer autocomplete
        // opens History and leaks into this document-level listener, but
        // its target is the composer textarea — outside the sheet — so it
        // is ignored. A deliberate restore (or an assistive/programmatic
        // Enter) targets the auto-focused search input inside the sheet and
        // is honored immediately, with nothing swallowed.
        if (!sheetRef.current?.contains(e.target as Node)) return;
        e.preventDefault();
        const item = f[idx];
        if (!item || restoreRef.current.gateOf(item)) return;
        if (!restoring) {
          setRestoringId(rowKey(item));
          restoreRef.current.restore(item).catch(() => setRestoringId(null));
        }
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  const onRowClick = async (item: Archived) => {
    if (restoringId || gateOf(item)) return;
    setRestoringId(rowKey(item));
    try {
      await restore(item);
    } catch {
      setRestoringId(null);
    }
  };

  return (
    <ModalSheet ref={sheetRef} onClose={close} label="Session history">
      {/* Search header */}
      <div className="hh">
        <div className="hh-search flex-center text-base">
          <Icon name="search" size={16} />
          <input
            className="text-base"
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search sessions…"
          />
        </div>
      </div>

      {/* Scrollable list */}
      <div className="history-list" ref={listRef}>
        {filtered.length === 0 ? (
          <div className="h-empty text-base">
            {query ? "No matches" : "No archived sessions yet"}
          </div>
        ) : (
          (() => {
            let flatIdx = 0;
            return groups.map(({ label, items }) => (
              <div key={label}>
                <div className="hg-h">
                  <span className="hg-d text-sm">{label}</span>
                  <span className="hg-n text-sm">{items.length}</span>
                </div>
                {items.map((item) => {
                  const myIdx = flatIdx++;
                  const a = item.agent;
                  if (!a.archive) return null;
                  const archive = a.archive;
                  const primary = archive.repos[0];
                  const repoLabel = primary ? basename(primary.repo_path) : a.name;
                  const branchLabel = primary?.branch_name ?? null;
                  const task = firstLine(a.task || "Untitled session", 96);
                  const adds = archive.diff_stats.additions;
                  const dels = archive.diff_stats.deletions;
                  const showStats = adds > 0 || dels > 0;
                  const when = formatHistoryTime(archive.archived_at);
                  const isRestoring = restoringId === rowKey(item);
                  const isFocused = focusedIndex === myIdx;
                  const rowGate = gateOf(item);

                  return (
                    <EngineScope key={rowKey(item)} hostId={item.hostId}>
                      <button
                        className={`hrow flex-center archived text-base${isFocused ? " focused" : ""}`}
                        onClick={() => onRowClick(item)}
                        onMouseEnter={() => setFocusedIndex(myIdx)}
                        disabled={!!restoringId || !!rowGate}
                        // The footer already carries the gate's reason; the row
                        // repeats it where the dead click happens.
                        title={rowGate ?? undefined}
                        // Dim the other rows during a restore, but keep the active
                        // row at full opacity so its spinner stays visible (a
                        // parent opacity would otherwise cap the child rule).
                        style={{ opacity: !isRestoring && restoringId ? 0.5 : undefined }}
                      >
                        <span className="hr-status iflex-center">
                          <Icon name="dot" size={6} />
                        </span>
                        <span className="hr-project truncate text-base">{repoLabel}</span>
                        <span className="hr-sep text-base">/</span>
                        <span className="hr-title truncate text-base">{task}</span>
                        {branchLabel && (
                          <>
                            <span className="hr-dot text-base">·</span>
                            <span className="hr-branch truncate text-sm">{branchLabel}</span>
                          </>
                        )}
                        <HostChip />
                        <span className="hr-spacer" />
                        {showStats && (
                          <span className="hr-diff iflex-center text-sm">
                            <span className="add">+{adds}</span>
                            <span className="rem">-{dels}</span>
                          </span>
                        )}
                        <span className="hr-date text-sm">{when}</span>
                        <span
                          className={`hr-goto iflex-center text-sm${isRestoring ? " restoring" : ""}`}
                        >
                          {isRestoring ? (
                            <>
                              <Loader variant="inherit" />
                              Restoring…
                            </>
                          ) : (
                            <>
                              <svg
                                width="11"
                                height="11"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                strokeWidth="2"
                                strokeLinecap="round"
                                strokeLinejoin="round"
                              >
                                <polyline points="9 18 15 12 9 6" />
                              </svg>
                              Restore
                            </>
                          )}
                        </span>
                      </button>
                    </EngineScope>
                  );
                })}
              </div>
            ));
          })()
        )}
      </div>

      {/* Footer */}
      <div className="history-foot flex-center text-xs">
        <span>
          <kbd className="kbd">Esc</kbd> <span className="dim">to close</span>
        </span>
        {restoreGate ? (
          <span className="dim">{restoreGate}</span>
        ) : (
          <span>
            <kbd className="kbd">↵</kbd> <span className="dim">to restore</span>
          </span>
        )}
      </div>
    </ModalSheet>
  );
}

// ── date grouping helpers ────────────────────────────────────────────────────

interface DateGroup {
  label: string;
  items: Archived[];
}

function groupByDate(records: Archived[]): DateGroup[] {
  const today = startOfLocalDay(new Date());
  const yesterday = today - 24 * 60 * 60 * 1000;
  const groups: Map<string, Archived[]> = new Map();
  const labels: string[] = [];

  for (const r of records) {
    const iso = r.agent.archive?.archived_at;
    const t = iso ? new Date(iso).getTime() : NaN;
    const day = Number.isNaN(t) ? "Unknown" : labelFor(t, today, yesterday);
    if (!groups.has(day)) {
      groups.set(day, []);
      labels.push(day);
    }
    const bucket = groups.get(day);
    if (bucket) bucket.push(r);
  }
  return labels.map((label) => ({ label, items: groups.get(label) ?? [] }));
}

function startOfLocalDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

function labelFor(ts: number, today: number, yesterday: number): string {
  const dayStart = startOfLocalDay(new Date(ts));
  if (dayStart === today) return "Today";
  if (dayStart === yesterday) return "Yesterday";
  const d = new Date(ts);
  const sameYear = d.getFullYear() === new Date().getFullYear();
  const month = d.toLocaleString(undefined, { month: "short" });
  return sameYear ? `${month} ${d.getDate()}` : `${month} ${d.getDate()}, ${d.getFullYear()}`;
}

function formatHistoryTime(iso: string): string {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "";
  const ageMs = Date.now() - t;
  const minutes = Math.floor(ageMs / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  const d = new Date(t);
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
