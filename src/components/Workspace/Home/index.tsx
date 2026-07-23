import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { ActionCard } from "./ActionCard";
import { greeting } from "./greeting";

/** Home — the center pane when no agent, run, or draft is selected and Mission
 *  Control is turned off (Developer setting). A calm landing surface that puts
 *  the next real action one click away and adapts to where the user is: a
 *  first-timer with no projects is pointed at adding one; a returning user gets
 *  a greeting and a New-agent hero. Every action here maps to a live store
 *  action — nothing decorative. */
export function Home() {
  const workspace = useAppStore((s) => s.workspace);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const createDraft = useAppStore((s) => s.createDraft);
  const addWorkspaceRepo = useAppStore((s) => s.addWorkspaceRepo);
  const setLastRepoPath = useAppStore((s) => s.setLastRepoPath);
  const lastRepoPath = useAppStore((s) => s.lastRepoPath);
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);
  const toggleHistory = useAppStore((s) => s.toggleHistory);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  const setLastError = useAppStore((s) => s.setLastError);

  const [adding, setAdding] = useState(false);

  const repos = workspace?.repos ?? [];
  const projects = workspace?.projects ?? [];
  const allAgents = workspace?.agents ?? [];
  const liveAgents = allAgents.filter((a) => !a.archive);
  const hasProjects = repos.length > 0;
  const hasAgents = liveAgents.length > 0;
  // History surfaces archived agents, so it's the presence of an archive — not
  // of live agents — that decides whether the card is worth showing.
  const hasArchived = allAgents.some((a) => a.archive);

  // Resolve the New-agent target repo the same way ⌘N does (see
  // util/shortcuts.ts): the last project an agent was started in if it still
  // exists, else the selected agent's project, else the first pinned repo.
  const recent = lastRepoPath && repos.includes(lastRepoPath) ? lastRepoPath : undefined;
  const targetRepo =
    recent ?? liveAgents.find((a) => a.id === selectedAgentId)?.repos[0]?.repo_path ?? repos[0];
  const targetName = projects.find((p) => p.path === targetRepo)?.name;

  const newAgent = () => {
    if (targetRepo) createDraft(targetRepo);
  };

  // Same flow as the sidebar's "Open a folder" (NewProjectPopover): native
  // directory picker → pin the repo → remember it as the next New-agent target.
  const addProject = async () => {
    if (adding) return;
    setAdding(true);
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "Select a git repository",
      });
      // `addWorkspaceRepo` routes its own failures to the error banner; the
      // dialog itself can still reject (plugin/platform error), which we'd
      // otherwise drop as an unhandled rejection with no user feedback.
      if (typeof picked === "string") {
        await addWorkspaceRepo(picked);
        setLastRepoPath(picked);
      }
    } catch (e) {
      setLastError(`Couldn't open the folder picker: ${String(e)}`);
    } finally {
      setAdding(false);
    }
  };

  const title = hasProjects ? greeting(new Date()) : "Welcome to Fletch";
  const sub = !hasProjects
    ? "Add a local git repository to spin up your first agent — each one gets its own isolated checkout and sandbox."
    : hasAgents
      ? "Pick up where you left off, or start something new."
      : `You're all set. Start your first agent${targetName ? ` in ${targetName}` : ""}.`;

  return (
    <div className="pane center">
      <div className="center-h flex-center">
        <IconButton
          tip={leftCollapsed ? "Show sidebar (⌘B)" : "Hide sidebar (⌘B)"}
          onClick={toggleLeft}
        >
          <Icon name="sidebarL" />
        </IconButton>
      </div>

      <div className="home-wrap flex-center fade-in">
        <div className="home-glow" aria-hidden="true" />
        <div className="home-inner">
          <div className="home-id">
            <span className="home-eyebrow">
              <span className="d" />
              Fletch
            </span>
            <h1 className="home-title serif">{title}</h1>
            <p className="home-sub">{sub}</p>
          </div>

          <div className="home-actions">
            {hasProjects ? (
              <ActionCard
                tone="primary"
                icon="sparkle"
                kbd="⌘ N"
                title="New agent"
                sub={targetName ? `Start a coding agent in ${targetName}` : "Start a coding agent"}
                onClick={newAgent}
              />
            ) : (
              <ActionCard
                tone="primary"
                icon="folder"
                title="Add your first project"
                sub="Open a local git repository on your machine"
                onClick={() => void addProject()}
                busy={adding}
              />
            )}

            <div className="home-grid">
              {hasProjects && (
                <ActionCard
                  icon="folder"
                  title="Add a project"
                  sub="Open another local repo"
                  onClick={() => void addProject()}
                  busy={adding}
                />
              )}
              {hasArchived && (
                <ActionCard
                  icon="history"
                  title="History"
                  sub="Revisit archived agents"
                  onClick={() => toggleHistory(true)}
                />
              )}
              <ActionCard
                icon="cube"
                title="Connect a provider"
                sub="Claude, Codex & more"
                onClick={() => openSettingsScreen("providers")}
              />
            </div>
          </div>

          {hasProjects && (
            <div className="home-tips flex-center">
              <Tip k="⌘ N" label="New agent" />
              <Tip k="⌘ K" label="Search" />
              <Tip k="⌘ B" label="Sidebar" />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function Tip({ k, label }: { k: string; label: string }) {
  return (
    <span className="home-tip iflex-center">
      <kbd className="home-kbd">{k}</kbd>
      <span>{label}</span>
    </span>
  );
}
