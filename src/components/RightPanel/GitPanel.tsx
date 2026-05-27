import { useEffect, useState } from "react";
import type { AgentRecord, FileStatus, GitState, PrState } from "../../api";
import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { IconButton } from "../ui/IconButton";
import { primaryFor, secondaryFor, type GitPanelState } from "./primaryActions";

function deriveState(git: GitState | null, pr: PrState | null): GitPanelState {
  if (!git) return "loading";
  if (git.files.some((f) => f.kind === "conflicted")) return "conflicts";
  if (pr?.state === "merged") return "merged";
  if (pr?.state === "open")   return "pr-open";
  if (pr?.state === "closed") return "pr-closed";
  if (git.files.length > 0)  return "changes";
  if (git.ahead > 0)         return "pushed";
  return "clean";
}

/** Status letter for the file badge — matches CSS `.gs.<kind>` selectors. */
function kindLabel(kind: FileStatus["kind"]): string {
  switch (kind) {
    case "modified":   return "M";
    case "added":      return "A";
    case "deleted":    return "D";
    case "renamed":    return "R";
    case "untracked":  return "?";
    case "conflicted": return "!";
    default:           return "?";
  }
}

// ── Sub-components ────────────────────────────────────────────────

function PRCard({ pr }: { pr: PrState }) {
  return (
    <div className="git-pr-card">
      <div className="gpc-title">{pr.title}</div>
      <div className="gpc-meta">
        <span>#{pr.number}</span>
        {pr.mergeable
          ? <span className="gpc-badge ready">Mergeable</span>
          : <span className="gpc-badge warn">Not mergeable</span>}
      </div>
      <a
        href={pr.url}
        target="_blank"
        rel="noreferrer"
        className="gpc-link"
      >
        <Icon name="github" size={11} />
        View on GitHub
      </a>
    </div>
  );
}

function ClosedPRCard({ pr }: { pr: PrState }) {
  return (
    <div className="git-pr-card">
      <div className="gpc-title">{pr.title}</div>
      <div className="gpc-meta">
        <span>#{pr.number}</span>
        <span className="gpc-badge warn">Closed</span>
      </div>
      <a href={pr.url} target="_blank" rel="noreferrer" className="gpc-link">
        <Icon name="github" size={11} />
        View on GitHub
      </a>
    </div>
  );
}

function MergedCard() {
  return (
    <div className="git-state-card merged">
      <Icon name="merge" size={14} />
      <span>Merged into base branch. Workspace can now be archived.</span>
    </div>
  );
}

function ConflictCard({ files }: { files: FileStatus[] }) {
  const conflicted = files.filter((f) => f.kind === "conflicted");
  const first = conflicted[0]?.path ?? "";
  const rest = conflicted.length - 1;
  return (
    <div className="git-state-card conflict">
      <Icon name="merge" size={14} />
      <span>
        Conflicts in <span className="mono">{first}</span>
        {rest > 0 && ` and ${rest} more`}.
      </span>
    </div>
  );
}

// ── Main panel ────────────────────────────────────────────────────

/** State-aware git panel driven by live git state from the Tauri backend.
 *  The panel is feature-flagged off by default in settings. */
export function GitPanel({ agent }: { agent: AgentRecord }) {
  const gitState = useAppStore((s) => s.gitStates[agent.id] ?? null);
  const prState  = useAppStore((s) => s.prStates[agent.id] ?? null);
  const fetchGitState = useAppStore((s) => s.fetchGitState);
  const fetchPrState  = useAppStore((s) => s.fetchPrState);
  const pushAgent  = useAppStore((s) => s.pushAgent);
  const pullAgent  = useAppStore((s) => s.pullAgent);
  const createPr   = useAppStore((s) => s.createPr);
  const mergePr    = useAppStore((s) => s.mergePr);
  const archive    = useAppStore((s) => s.archive);

  useEffect(() => {
    void fetchGitState(agent.id);
    void fetchPrState(agent.id);
  }, [agent.id, fetchGitState, fetchPrState]);

  const panelState = deriveState(gitState, prState);

  const [selected, setSelected] = useState<string | null>(null);
  useEffect(() => {
    setSelected((prev) => {
      const paths = gitState?.files.map((f) => f.path) ?? [];
      if (prev && paths.includes(prev)) return prev;
      return paths[0] ?? null;
    });
  }, [gitState]);

  const [moreOpen, setMoreOpen] = useState(false);

  function handlePrimaryClick() {
    switch (panelState) {
      case "pushed":
      case "pr-closed":
        void createPr(agent.id, "", "");
        break;
      case "pr-open":
        if (prState?.url) window.open(prState.url, "_blank");
        break;
      case "merged":
        void archive(agent.id);
        break;
      default:
        break;
    }
  }

  function handleSecondaryClick(key: string) {
    setMoreOpen(false);
    switch (key) {
      case "push":       void pushAgent(agent.id);             break;
      case "pull":       void pullAgent(agent.id);             break;
      case "open-pr":    void createPr(agent.id, "", "");      break;
      case "merge":      void mergePr(agent.id);               break;
      case "view-pr":    prState?.url && window.open(prState.url, "_blank"); break;
      case "archive":    void archive(agent.id);               break;
      default:           break;
    }
  }

  const counts = {
    files:    gitState?.files.length ?? 0,
    ahead:    gitState?.ahead ?? 0,
    prNumber: prState?.number,
  };
  const primary   = primaryFor(panelState, counts);
  const secondary = secondaryFor(panelState);

  const branch   = gitState?.branch || agent.repos[0]?.branch || "(no branch yet)";
  const base     = gitState?.parent_branch || agent.repos[0]?.parent_branch || "main";
  // Show the file list only when there are actually uncommitted files to display.
  // "pushed" and "pr-closed" have no local changes (files already committed).
  const showFiles  = panelState === "changes" || panelState === "conflicts";
  const showCommit = panelState === "changes";

  return (
    <>
      {/* Branch + stats */}
      <div className="git-state">
        <div className="git-branch-row">
          <Icon name="branch" />
          <span>{branch}</span>
          <span className="base">← {base}</span>
        </div>
        <div className="git-stats">
          <span><span className="num">{gitState?.ahead ?? 0}</span> ahead</span>
          <span><span className="num">{gitState?.behind ?? 0}</span> behind</span>
          {((gitState?.additions ?? 0) > 0 || (gitState?.deletions ?? 0) > 0) && (
            <>
              <span><span className="add">+{gitState!.additions}</span></span>
              <span><span className="rem">−{gitState!.deletions}</span></span>
            </>
          )}
        </div>
      </div>

      {/* Primary action + overflow menu */}
      <div className="git-primary">
        <div className="status-line">
          <span className={`d ${primary.statusKind}`} />
          <span>{primary.statusLabel}</span>
          {primary.statusExtra && (
            <span
              style={{
                color: "var(--fg-3)", marginLeft: "auto",
                fontFamily: "var(--font-mono)", letterSpacing: 0,
                textTransform: "none", fontWeight: 400, fontSize: 11,
              }}
            >
              {primary.statusExtra}
            </span>
          )}
        </div>
        <div className="actions">
          <button
            type="button"
            disabled={panelState === "loading" || panelState === "changes" || panelState === "conflicts" || panelState === "clean"}
            className={`btn-t ${primary.danger ? "outline" : "primary"}`}
            style={primary.danger ? { borderColor: "var(--danger)", color: "var(--danger)" } : undefined}
            onClick={handlePrimaryClick}
          >
            <Icon name={primary.icon} />
            {primary.label}
          </button>
          {secondary.length > 0 && (
            <div className="more">
              <IconButton tip="More actions" onClick={() => setMoreOpen((v) => !v)}>
                <Icon name="more" />
              </IconButton>
              {moreOpen && (
                <>
                  <div
                    style={{ position: "fixed", inset: 0, zIndex: 199 }}
                    onClick={() => setMoreOpen(false)}
                  />
                  <div className="dd" style={{ top: "calc(100% + 6px)", right: 0, minWidth: 200 }}>
                    {secondary.map((s) => (
                      <div key={s.key} className="dd-item" onClick={() => handleSecondaryClick(s.key)}>
                        <div className="di-i"><Icon name={s.icon} size={12} /></div>
                        <span className="di-l">{s.label}</span>
                        {s.kbd && <span className="di-m">{s.kbd}</span>}
                      </div>
                    ))}
                  </div>
                </>
              )}
            </div>
          )}
        </div>
      </div>

      {/* Changed file list */}
      {showFiles && (
        <>
          <div className="git-files-h">
            <span>Changes · {gitState?.files.length ?? 0}</span>
            <div className="actions">
              <IconButton size="xs" tip="Stage all"><Icon name="check" /></IconButton>
              <IconButton size="xs" tip="Refresh"><Icon name="refresh" /></IconButton>
            </div>
          </div>
          <div style={{ flex: 1, minHeight: 0, overflowY: "auto" }}>
            {(gitState?.files ?? []).map((f) => (
              <div
                key={f.path}
                className={`git-file ${selected === f.path ? "active" : ""}`}
                onClick={() => setSelected(f.path)}
              >
                <span className={`gs ${f.kind}`}>{kindLabel(f.kind)}</span>
                <span className="gn">{f.path}</span>
                <span className="gx">
                  {f.additions > 0 && <span className="add">+{f.additions}</span>}
                  {f.deletions > 0 && <span className="rem">−{f.deletions}</span>}
                </span>
              </div>
            ))}
          </div>
        </>
      )}

      {/* Commit message card — shown when there are uncommitted changes */}
      {showCommit && (
        <div className="git-commit">
          <div className="cm-title">Commit message · auto-drafted</div>
          <div className="cm-card">
            <div className="ct">Auto-draft not yet available</div>
            <div className="cb" />
          </div>
          <div className="cm-foot">
            <IconButton size="xs" tip="Regenerate"><Icon name="sparkle" /></IconButton>
            <IconButton size="xs" tip="Edit message"><Icon name="edit" /></IconButton>
            <span className="grow" />
            <span className="hint">⌘↵ to commit + push</span>
          </div>
        </div>
      )}

      {/* State-specific bottom cards */}
      {panelState === "pr-open"   && prState  && <PRCard pr={prState} />}
      {panelState === "pr-closed" && prState  && <ClosedPRCard pr={prState} />}
      {panelState === "merged"    && <MergedCard />}
      {panelState === "conflicts" && gitState  && <ConflictCard files={gitState.files} />}

      {/* Empty / loading states */}
      {panelState === "loading" && (
        <div className="empty-msg" style={{ marginTop: "auto" }}>
          <div className="et">Loading…</div>
          <div>Fetching git state.</div>
        </div>
      )}
      {panelState === "clean" && (
        <div className="empty-msg" style={{ marginTop: "auto" }}>
          <div className="et">All clean</div>
          <div>No uncommitted changes. Type a follow-up to start working.</div>
        </div>
      )}
    </>
  );
}
