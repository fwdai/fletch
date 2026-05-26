import { useState } from "react";
import type { AgentRecord } from "../../api";
import { MOCK_GIT_FILES, MOCK_COMMIT_MESSAGE, type MockGitState } from "../../data/mocks";
import { Icon } from "../Icon";
import { IconButton } from "../ui/IconButton";
import { primaryFor, secondaryFor } from "./primaryActions";

/** State-aware git panel. The single primary action morphs based on
 *  the current state; secondary actions live in the "…" menu. All
 *  data here is mocked until git IPC commands land — the panel is
 *  feature-flagged off by default in settings. */
export function GitPanel({ agent, state }: { agent: AgentRecord; state: MockGitState }) {
  const [selected, setSelected] = useState(MOCK_GIT_FILES[0]?.path);
  const [moreOpen, setMoreOpen] = useState(false);

  const primary = primaryFor(state);
  const secondary = secondaryFor(state);
  const branch = agent.repos[0]?.branch ?? "(no branch yet)";
  const base = agent.repos[0]?.parent_branch ?? "main";
  const showFiles = state !== "clean" && state !== "merged";
  const showCommit = state === "changes";

  return (
    <>
      <div className="git-state">
        <div className="git-branch-row">
          <Icon name="branch" />
          <span>{branch}</span>
          <span className="base">← {base}</span>
        </div>
        <div className="git-stats">
          <span><span className="num">{state === "pushed" || state === "pr-open" ? 1 : 0}</span> ahead</span>
          <span><span className="num">0</span> behind</span>
        </div>
      </div>

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
            className={`btn-t ${primary.danger ? "outline" : "primary"}`}
            style={
              primary.danger
                ? { borderColor: "var(--danger)", color: "var(--danger)" }
                : undefined
            }
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
                  <div
                    className="dd"
                    style={{ top: "calc(100% + 6px)", right: 0, minWidth: 200 }}
                  >
                    {secondary.map((s) => (
                      <div key={s.label} className="dd-item" onClick={() => setMoreOpen(false)}>
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

      {showFiles && (
        <>
          <div className="git-files-h">
            <span>Changes · {MOCK_GIT_FILES.length}</span>
            <div className="actions">
              <IconButton size="xs" tip="Stage all"><Icon name="check" /></IconButton>
              <IconButton size="xs" tip="Refresh"><Icon name="refresh" /></IconButton>
            </div>
          </div>
          <div style={{ flex: 1, minHeight: 0 }}>
            {MOCK_GIT_FILES.map((f) => (
              <div
                key={f.path}
                className={`git-file ${selected === f.path ? "active" : ""}`}
                onClick={() => setSelected(f.path)}
              >
                <span className={`gs ${f.status}`}>{f.status}</span>
                <span className="gn">{f.path}</span>
                <span className="gx">
                  {f.add > 0 && <span className="add">+{f.add}</span>}
                  {f.rem > 0 && <span className="rem">−{f.rem}</span>}
                </span>
              </div>
            ))}
          </div>
        </>
      )}

      {showCommit && (
        <div className="git-commit">
          <div className="cm-title">Commit message · auto-drafted</div>
          <div className="cm-card">
            <div className="ct">{MOCK_COMMIT_MESSAGE.title}</div>
            <div className="cb">{MOCK_COMMIT_MESSAGE.body}</div>
          </div>
          <div className="cm-foot">
            <IconButton size="xs" tip="Regenerate"><Icon name="sparkle" /></IconButton>
            <IconButton size="xs" tip="Edit message"><Icon name="edit" /></IconButton>
            <span className="grow" />
            <span className="hint">⌘↵ to commit + push</span>
          </div>
        </div>
      )}

      {state === "clean" && (
        <div className="empty-msg" style={{ marginTop: "auto" }}>
          <div className="et">All clean</div>
          <div>No uncommitted changes. Type a follow-up to start working.</div>
        </div>
      )}
    </>
  );
}
