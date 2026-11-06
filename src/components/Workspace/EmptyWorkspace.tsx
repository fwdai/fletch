import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/api";
import { Composer } from "@/components/Composer";
import { BranchPicker } from "@/components/Composer/BranchPicker";
import { type ProjectOption, ProjectPicker } from "@/components/Composer/ProjectPicker";
import { WorkspaceTypePicker } from "@/components/Composer/WorkspaceTypePicker";
import { declaredWorkspaceType } from "@/components/Composer/workspaceType";
import { Icon, LandmarkGlyph } from "@/components/Icon";
import { PanelToggle } from "@/components/PanelToggle";
import { ProjectIcon } from "@/components/ProjectIcon";
import { IconButton } from "@/components/ui/IconButton";
import { getLinearTeamId } from "@/storage/projectSettings";
import type { DraftAgent } from "@/store";
import { useAppStore } from "@/store";
import { activeEntry, useGate } from "@/store/capabilities";
import { workspaceTypeOptionsFor } from "@/store/workspaceTypes";
import { WorkflowComposer, WorkflowHeading } from "@/workflows/run/WorkflowComposer";
import { creationNote } from "./creationNote";

/** Empty-state pane shown when the user has started a draft agent.
 *  First message in the composer spawns the real agent and dispatches
 *  the message in one go. */
export function EmptyWorkspace({ draft }: { draft: DraftAgent }) {
  const rerollDraftName = useAppStore((s) => s.rerollDraftName);
  const spawnFromDraft = useAppStore((s) => s.spawnFromDraft);
  const removeDraft = useAppStore((s) => s.removeDraft);
  const updateDraft = useAppStore((s) => s.updateDraft);
  const setNewDraftSelection = useAppStore((s) => s.setNewDraftSelection);
  const setLastRepoPath = useAppStore((s) => s.setLastRepoPath);
  const rememberDraftBase = useAppStore((s) => s.rememberDraftBase);
  const resolveDraftBase = useAppStore((s) => s.resolveDraftBase);
  // The project the user picked most recently, so an in-flight base resolve for
  // a project they've since switched away from can be discarded rather than
  // applied out of order (see the ProjectPicker's onChange).
  const pickedRepoPath = useRef<string | null>(null);
  const projectRefs = useAppStore((s) => s.workspace?.projects ?? []);
  // One picker entry per project: a multi-repo project shows once, valued at
  // its primary (first) repo, which is where the agent spawns.
  const projectOptions = useMemo(() => {
    const seen = new Map<string, ProjectOption>();
    for (const ref of projectRefs) {
      const key = ref.project_id || ref.path;
      if (!seen.has(key)) seen.set(key, { path: ref.path, label: ref.name });
    }
    return [...seen.values()];
  }, [projectRefs]);
  // How many repos the drafted project spans — a multi-repo project gets a
  // checkout of each at spawn, and the sub-copy should say so.
  const repoCount = useMemo(() => {
    const projectId = projectRefs.find((r) => r.path === draft.repoPath)?.project_id;
    if (!projectId) return 1;
    return projectRefs.filter((r) => r.project_id === projectId).length;
  }, [projectRefs, draft.repoPath]);
  const runLocalCommand = useAppStore((s) => s.runLocalCommand);

  // The project this draft targets — keys its Linear team and default workflow
  // (per-project `project_settings`).
  const project = projectRefs.find((r) => r.path === draft.repoPath);
  const projectId = project?.project_id ?? "";

  // The project's configured Linear team (Project Settings), scoping the
  // composer's issue picker. Undefined while loading or when none is set —
  // the picker then serves GitHub issues only.
  const [linearTeamId, setLinearTeamId] = useState<string | undefined>();
  useEffect(() => {
    let cancelled = false;
    setLinearTeamId(undefined);
    getLinearTeamId(projectId)
      .then((teamId) => {
        if (!cancelled) setLinearTeamId(teamId);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  // A host answers no `wf_*` op (docs/multi-host-plan.md §5.3, item 2), so on a
  // remote environment a workflow draft gets the quick-agent page — which is
  // all a host can run.
  const workflowGate = useGate("workflows");
  const workflowMode = draft.kind === "workflow" && !workflowGate;
  // The workspace type is offered only for an agent on this Mac: a paired
  // host's spawn carries no workspace type yet. `spawnFromDraft` sends what
  // the chip declares on the same rule.
  const workspaceTypeShown = useAppStore((s) => activeEntry(s).kind !== "remote");
  // What the page says will be created follows the sandbox the pill declares:
  // the same derivation the spawn sends, so the sentence can't promise one
  // sandbox while the agent lands in the other.
  const workspaceTypeOptions = useAppStore((s) =>
    workspaceTypeOptionsFor(s, draft.repoPath, draft.provider),
  );
  const note = creationNote(
    workspaceTypeShown ? declaredWorkspaceType(workspaceTypeOptions, draft.workspaceType) : null,
    repoCount,
  );

  return (
    <div className="pane center">
      <div className="center-h flex-center">
        <PanelToggle side="left" />
        <div className="task" style={{ flexDirection: "row", alignItems: "center", gap: 10 }}>
          {project && <ProjectIcon project={project} />}
          <span
            style={{
              fontSize: "var(--fs-xs)",
              color: "var(--fg-3)",
              fontFamily: "var(--font-mono)",
              textTransform: "uppercase",
              letterSpacing: "0.08em",
            }}
          >
            Drafting
          </span>
          <span className="serif" style={{ fontSize: "var(--fs-lg)", color: "var(--accent)" }}>
            {draft.name}
          </span>
        </div>
        <IconButton tip="Discard draft" onClick={() => removeDraft(draft.id)}>
          <Icon name="close" />
        </IconButton>
      </div>

      <div className="empty-wrap flex-center fade-in">
        <div className="empty-body fade-in">
          <div className="empty-id flex-center">
            <div className="empty-mark flex-center text-xs">
              <span className="d" />
              <span>NEW WORKSPACE</span>
            </div>
            {/* A click rerolls the name; the glyph past its end says so on
                hover. */}
            <div
              className="empty-name text-5xl tip"
              data-tip="Click to reroll the name"
              onClick={() => rerollDraftName(draft.id)}
            >
              <LandmarkGlyph
                name={draft.name}
                size={36}
                strokeWidth={1.1}
                style={{ display: "inline-block", verticalAlign: "-6px", marginRight: 6 }}
              />
              {draft.name}
              <Icon name="refresh" size={16} className="empty-name-reroll" />
            </div>
          </div>

          {workflowMode ? (
            <WorkflowHeading />
          ) : (
            <>
              <h1 className="empty-title text-5xl">What should be the first task?</h1>
              <p className="empty-sub text-base">
                {note.kind === "blocked" ? (
                  note.text
                ) : (
                  <>
                    {note.before}
                    <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-1)" }}>
                      ~/.fletch/workspaces/{draft.name}
                    </span>
                  </>
                )}
              </p>
            </>
          )}

          <div className="empty-composer">
            {workflowMode ? (
              <WorkflowComposer
                repoPath={draft.repoPath}
                baseBranch={draft.base}
                name={draft.name}
                projectId={projectId}
                onLaunched={() => removeDraft(draft.id)}
              />
            ) : (
              <Composer
                // Remount when the target repo changes so the @/# mention
                // sources drop the previous repo's cached files/PRs (they only
                // refetch on menu-open, not on a repoPath change under a live
                // composer) — otherwise a wrong-repo file/PR could be inserted.
                key={draft.repoPath}
                autoFocus
                minRows={2}
                draftKey={draft.id}
                defaultProvider={draft.provider}
                projectDir={draft.repoPath}
                onLocalCommand={(action) => runLocalCommand(action)}
                mentionSource={() => api.listRepoTree(draft.repoPath)}
                listDir={api.listDir}
                listPrs={() => api.listRepoPrs(draft.repoPath)}
                listIssues={() => api.listTrackerIssues(draft.repoPath, linearTeamId)}
                listIssueComments={(issue) =>
                  api.issueComments(draft.repoPath, issue.source, issue.key)
                }
                onPickIssue={(issue) => updateDraft(draft.id, { issueRef: issue.key })}
                defaultModel={draft.model}
                defaultCustomAgentId={draft.customAgentId}
                onChangeSelection={(provider, model, customAgentId) => {
                  // An account belongs to its provider: another provider
                  // starts over from its own active account in Settings.
                  const account = provider === draft.provider ? draft.account : undefined;
                  updateDraft(draft.id, { provider, model, customAgentId, account });
                  setNewDraftSelection(provider, model, customAgentId);
                }}
                // A per-draft pick, never remembered for the next draft.
                account={{
                  current: draft.account,
                  onPick: (account) => updateDraft(draft.id, { account }),
                }}
                placeholder="Describe the task for the agent. ↵ to spawn."
                folders
                onSend={({
                  text,
                  provider,
                  model,
                  attachments,
                  thinking,
                  customAgentId,
                  folders,
                }) =>
                  spawnFromDraft(
                    draft.id,
                    text,
                    provider,
                    model,
                    attachments,
                    thinking,
                    customAgentId,
                    folders,
                  )
                }
              />
            )}
            <div className="empty-meta flex-center">
              <ProjectPicker
                value={draft.repoPath}
                projects={projectOptions}
                onChange={async (repoPath) => {
                  // Switching projects: the previously chosen base branch belongs
                  // to the old repo and may not exist here, so re-resolve against
                  // the new one — its own remembered pick, else its *default*
                  // (not a hardcoded "main") — and drop any issue tag, which
                  // belongs to the previous project's tracker (its key would
                  // close the wrong issue, or a dead one, from the new repo's
                  // PR). The brief text stays: it's visible and editable, unlike
                  // the hidden ref.
                  //
                  // Resolved before the switch is applied, so the draft is never
                  // momentarily pointing at a base from the previous repo — a
                  // spawn in that window would fork from a branch this repo may
                  // not even have. These are local ref reads, not network calls,
                  // but they're still subprocesses: switching again while one is
                  // in flight leaves two resolves racing, and the *earlier* one
                  // finishing last would strand the draft — and the spawn that
                  // follows — on the project the user just navigated away from.
                  // Only the newest selection may commit.
                  pickedRepoPath.current = repoPath;
                  const base = await resolveDraftBase(repoPath);
                  if (pickedRepoPath.current !== repoPath) return;
                  updateDraft(draft.id, { repoPath, base, issueRef: undefined });
                  setLastRepoPath(repoPath);
                }}
              />
              <BranchPicker
                repoPath={draft.repoPath}
                value={draft.base}
                onChange={(branch) => {
                  updateDraft(draft.id, { base: branch });
                  // Sticky per project: the next new agent here starts on this
                  // branch. Remembered on pick rather than on spawn, matching
                  // the provider/model selection above — a draft the user
                  // abandons still says what they meant to fork from.
                  rememberDraftBase(draft.repoPath, branch);
                }}
              />
              {/* Where the workspace runs, for a quick agent only: a box runs
                  no workflow steps. Per draft, an explicit pick kept as
                  made. */}
              {!workflowMode && workspaceTypeShown && (
                <WorkspaceTypePicker
                  repoPath={draft.repoPath}
                  provider={draft.provider}
                  current={draft.workspaceType}
                  onPick={(workspaceType) => updateDraft(draft.id, { workspaceType })}
                />
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
