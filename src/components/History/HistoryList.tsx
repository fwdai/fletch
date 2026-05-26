import { useMemo, useState } from "react";
import type { AgentRecord } from "../../api";
import { useAppStore } from "../../store";
import { basename, firstLine } from "../../util/format";
import { Icon, LandmarkGlyph } from "../Icon";
import { IconButton } from "../ui/IconButton";

/** Top-level list view for History. Filters + groups archived agents
 *  by archive date, descending. */
export function HistoryList() {
  const workspace = useAppStore((s) => s.workspace);
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const toggleHistory = useAppStore((s) => s.toggleHistory);
  const selectHistoryAgent = useAppStore((s) => s.selectHistoryAgent);

  const [query, setQuery] = useState("");

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
      if (branch && branch.toLowerCase().includes(q)) return true;
      return false;
    });
  }, [archived, query]);

  const groups = useMemo(() => groupByDate(filtered), [filtered]);

  return (
    <div className="pane center">
      <div className="center-h">
        <IconButton
          tip={leftCollapsed ? "Show sidebar (⌘B)" : "Hide sidebar (⌘B)"}
          onClick={toggleLeft}
        >
          <Icon name="sidebarL" />
        </IconButton>
        <div className="task">
          <div className="t-name">
            <Icon name="history" size={14} />
            <span>History</span>
          </div>
          <div className="t-meta">
            {archived.length} archived {archived.length === 1 ? "session" : "sessions"}
          </div>
        </div>
        <IconButton tip="Close history" onClick={() => toggleHistory(false)}>
          <Icon name="close" />
        </IconButton>
      </div>

      <div className="history-search">
        <Icon name="search" size={13} />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Filter by task, agent, repo, branch…"
          autoFocus
        />
      </div>

      <div className="history-scroll">
        {filtered.length === 0 ? (
          <div className="empty-msg" style={{ margin: "60px auto", maxWidth: 360 }}>
            <div className="et">
              {query ? "No matches" : "No archived sessions yet"}
            </div>
            <div>
              {query
                ? "Try a different search."
                : "Stop an agent and click Archive to keep its conversation here."}
            </div>
          </div>
        ) : (
          groups.map(({ label, items }) => (
            <div key={label} className="history-group">
              <div className="history-group-h">{label}</div>
              {items.map((a) => (
                <HistoryRow
                  key={a.id}
                  agent={a}
                  onClick={() => selectHistoryAgent(a.id)}
                />
              ))}
            </div>
          ))
        )}
      </div>
    </div>
  );
}

interface HistoryRowProps {
  agent: AgentRecord;
  onClick: () => void;
}

function HistoryRow({ agent, onClick }: HistoryRowProps) {
  const archive = agent.archive!;
  const primary = archive.repos[0];
  const repoLabel = primary ? basename(primary.repo_path) : null;
  const branchLabel = primary?.branch_name ?? null;
  const task = firstLine(agent.task || "Untitled session", 96);
  const adds = archive.diff_stats.additions;
  const dels = archive.diff_stats.deletions;
  const showStats = adds > 0 || dels > 0;
  const when = formatHistoryTime(archive.archived_at);

  return (
    <div className="history-row" onClick={onClick} role="button" tabIndex={0}>
      <span className="hr-dot" />
      <span className="hr-glyph">
        <LandmarkGlyph name={agent.name} />
      </span>
      <div className="hr-body">
        <div className="hr-line1">
          <span className="hr-name">{agent.name}</span>
          <span className="hr-task">{task}</span>
        </div>
        <div className="hr-line2">
          {repoLabel && <span className="hr-repo">{repoLabel}</span>}
          {branchLabel && <span className="hr-branch">{branchLabel}</span>}
        </div>
      </div>
      {showStats && (
        <div className="hr-stats">
          <span className="hr-add">+{adds}</span>
          <span className="hr-del">−{dels}</span>
        </div>
      )}
      <div className="hr-time" title={archive.archived_at}>
        {when}
      </div>
    </div>
  );
}

// ── grouping helpers ────────────────────────────────────────────────────────

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
    groups.get(day)!.push(r);
  }
  return labels.map((label) => ({ label, items: groups.get(label)! }));
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
