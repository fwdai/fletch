import { useMemo } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../store";
import type { AgentRecord } from "../api";
import { AgentRow } from "./AgentRow";

function basename(p: string): string {
  const parts = p.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

interface RepoGroup {
  repoPath: string;
  agents: AgentRecord[];
  pinned: boolean;
}

/** Sidebar: list of repos with their agents nested underneath.
 *
 *  Groups = union of (a) repos the user pinned via "+ Repo" and (b)
 *  every distinct primary repo (`agent.repos[0].repo_path`) across
 *  existing agents. So "removing" a pinned repo that still has agents
 *  visually keeps it (sourced from its agents) — the remove button is
 *  only meaningful for empty groups. */
function groupAgentsByPrimary(
  pinned: string[],
  agents: readonly AgentRecord[],
): RepoGroup[] {
  const map = new Map<string, RepoGroup>();
  for (const p of pinned) {
    map.set(p, { repoPath: p, agents: [], pinned: true });
  }
  for (const a of agents) {
    const primary = a.repos[0]?.repo_path;
    if (!primary) continue;
    const existing = map.get(primary);
    if (existing) {
      existing.agents.push(a);
    } else {
      map.set(primary, { repoPath: primary, agents: [a], pinned: false });
    }
  }
  return Array.from(map.values());
}

export function Sidebar() {
  const workspace = useAppStore((s) => s.workspace);
  const busy = useAppStore((s) => s.busy);
  const addWorkspaceRepo = useAppStore((s) => s.addWorkspaceRepo);

  const groups = useMemo(
    () => groupAgentsByPrimary(workspace?.repos ?? [], workspace?.agents ?? []),
    [workspace?.repos, workspace?.agents],
  );

  async function onAddRepo() {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Select a git repository",
    });
    if (typeof picked === "string") {
      await addWorkspaceRepo(picked);
    }
  }

  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <span className="sidebar-title">Repos</span>
        <button
          className="primary"
          onClick={onAddRepo}
          disabled={busy}
          title="Add a git repo to the sidebar"
        >
          + Repo
        </button>
      </div>
      <div className="list">
        {groups.length === 0 ? (
          <div className="empty">Add a repo to get started.</div>
        ) : (
          groups.map((g) => (
            <RepoGroupView
              key={g.repoPath}
              label={basename(g.repoPath)}
              repoPath={g.repoPath}
              agents={g.agents}
              removable={g.pinned && g.agents.length === 0}
            />
          ))
        )}
      </div>
    </aside>
  );
}

function RepoGroupView({
  label,
  repoPath,
  agents,
  removable,
}: {
  label: string;
  repoPath: string;
  agents: AgentRecord[];
  removable: boolean;
}) {
  const spawn = useAppStore((s) => s.spawn);
  const busy = useAppStore((s) => s.busy);
  const selectAgent = useAppStore((s) => s.selectAgent);
  const removeWorkspaceRepo = useAppStore((s) => s.removeWorkspaceRepo);

  async function onSpawn() {
    const rec = await spawn("custom", repoPath);
    if (rec) selectAgent(rec.id);
  }

  return (
    <div className="repo-group">
      <div className="repo-group-header" title={repoPath}>
        <span className="repo-group-name">{label}</span>
        <span className="repo-group-actions">
          <button
            type="button"
            className="ghostbtn"
            onClick={onSpawn}
            disabled={busy}
            title="Spawn an agent in this repo"
          >
            + Spawn
          </button>
          {removable && (
            <button
              type="button"
              className="ghostbtn"
              onClick={() => removeWorkspaceRepo(repoPath)}
              title="Remove this repo from the sidebar"
              aria-label="Remove repo"
            >
              ×
            </button>
          )}
        </span>
      </div>
      {agents.length === 0 ? (
        <div className="repo-group-empty">No agents yet.</div>
      ) : (
        agents.map((a) => <AgentRow key={a.id} agent={a} />)
      )}
    </div>
  );
}
