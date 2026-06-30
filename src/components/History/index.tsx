import { useEffect, useMemo, useRef, useState } from "react";
import type { AgentRecord } from "../../api";
import { useAppStore } from "../../store";
import { basename, firstLine } from "../../util/format";
import { Icon } from "../Icon";

export function History() {
  const workspace = useAppStore((s) => s.workspace);
  const toggleHistory = useAppStore((s) => s.toggleHistory);
  const restore = useAppStore((s) => s.restore);
  const [query, setQuery] = useState("");
  const [restoringId, setRestoringId] = useState<string | null>(null);
  const [focusedIndex, setFocusedIndex] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  // Mutable refs so document-level handler always sees latest values
  const filteredRef = useRef<AgentRecord[]>([]);
  const focusedIndexRef = useRef(0);
  const restoringIdRef = useRef<string | null>(null);

  const archived = useMemo(() => {
    const list = (workspace?.agents ?? []).filter((a) => a.archive);
    list.sort((a, b) => {
      const ta = a.archive?.archived_at ?? "";
      const tb = b.archive?.archived_at ?? "";
      return tb.localeCompare(ta);
    });
    return list;
  }, [workspace?.agents]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return archived;
    return archived.filter((a) => {
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

  // Keyboard: Esc, ↑↓ navigation, Enter to restore
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const f = filteredRef.current;
      const idx = focusedIndexRef.current;
      const restoring = restoringIdRef.current;

      if (e.key === "Escape") {
        toggleHistory(false);
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        setFocusedIndex(Math.min(idx + 1, f.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setFocusedIndex(Math.max(idx - 1, 0));
      } else if (e.key === "Enter") {
        e.preventDefault();
        const item = f[idx];
        if (item && !restoring) {
          setRestoringId(item.id);
          restore(item.id).catch(() => setRestoringId(null));
        }
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [toggleHistory, restore]);

  const onRowClick = async (id: string) => {
    if (restoringId) return;
    setRestoringId(id);
    try {
      await restore(id); // restore already sets historyOpen: false in the store
    } catch {
      setRestoringId(null);
    }
  };

  return (
    <div className="history-overlay" onClick={() => toggleHistory(false)}>
      <div className="history-sheet" onClick={(e) => e.stopPropagation()}>
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
                  {items.map((a) => {
                    const myIdx = flatIdx++;
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
                    const isRestoring = restoringId === a.id;
                    const isFocused = focusedIndex === myIdx;

                    return (
                      <button
                        key={a.id}
                        className={`hrow flex-center archived text-base${isFocused ? " focused" : ""}`}
                        onClick={() => onRowClick(a.id)}
                        onMouseEnter={() => setFocusedIndex(myIdx)}
                        disabled={!!restoringId}
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
                              <span className="dots">
                                <i />
                                <i />
                                <i />
                              </span>
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
          <span>
            <kbd className="kbd">↵</kbd> <span className="dim">to restore</span>
          </span>
        </div>
      </div>
    </div>
  );
}

// ── date grouping helpers ────────────────────────────────────────────────────

interface DateGroup {
  label: string;
  items: AgentRecord[];
}

function groupByDate(records: AgentRecord[]): DateGroup[] {
  const today = startOfLocalDay(new Date());
  const yesterday = today - 24 * 60 * 60 * 1000;
  const groups: Map<string, AgentRecord[]> = new Map();
  const labels: string[] = [];

  for (const r of records) {
    const iso = r.archive?.archived_at;
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
