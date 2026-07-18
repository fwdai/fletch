// run/WorkflowComposer.tsx — the "Workflow" composer block. It renders ONLY the
// prompt box (the host's new-agent page provides the identity, headings, and
// project/branch pickers around it, identical to the agent screen). It reuses
// the agent composer's shared input core (ComposerFrame + useComposerInput), so
// slash commands, @-file mentions, and #-PR mentions behave identically here;
// only the footer (a workflow picker instead of a model picker) and the submit
// (wf_launch instead of a chat send) differ.
//
// Launch goes through the v1 scheduler: `wf_launch` takes the definition's spec
// snapshot (plus any @-staged attachments) and returns the run id, which the
// monitor (RunView) then observes.

import { type CSSProperties, Fragment, useRef, useState } from "react";
import { api } from "../../api";
import { ComposerFrame } from "../../components/Composer/ComposerFrame";
import { useComposerInput } from "../../components/Composer/useComposerInput";
import { Icon } from "../../components/Icon";
import { Chip } from "../../components/ui/Chip";
import { DEFAULT_PROVIDER_ID } from "../../data/providers";
import { useAppStore } from "../../store";
import { AgentAvatar } from "../builder/AgentAvatar";
import { resolveAlias } from "../shared";
import type { Definition, Spec } from "../spec";
import { rememberDefaultWorkflow } from "./projectPipeline";
import { flattenSteps } from "./RunView/flatten";
import { useDefinitions } from "./useDefinitions";

const FLOW_HUE = 285;

/** Context the host's new-agent page hands to the workflow composer/heading. */
export interface ComposerContext {
  repoPath: string;
  baseBranch: string;
  name: string;
  /** Project id for per-project prefs (remembering the picked default flow). */
  projectId?: string;
  /** The project's remembered default workflow — preselected when the user
   *  hasn't picked another this session. */
  defaultWorkflowId?: string | null;
  /** GitHub issue number (as text) this launch was started from (Home inbox
   *  "Start work" → Pipeline). Threaded onto the run so its finalized PR
   *  closes the issue. Undefined for a normal launch. */
  issueRef?: string;
}

/** Heading slot for the workflow mode — replaces the agent's title/subtitle on
 *  the (otherwise shared) new-agent page. */
export function WorkflowHeading() {
  return (
    <>
      <h1 className="empty-title">What should the workflow do?</h1>
      <p className="empty-sub">
        A chain of agents runs on one branch — each step forks from the last.
      </p>
    </>
  );
}

export function WorkflowComposer({
  repoPath,
  baseBranch,
  projectId,
  defaultWorkflowId,
  issueRef,
}: ComposerContext) {
  const selectRun = useAppStore((s) => s.selectRun);
  const setLastError = useAppStore((s) => s.setLastError);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  const customAgents = useAppStore((s) => s.customAgents);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);

  const { definitions, loading } = useDefinitions();
  // "" until the user picks this session — then the project's remembered default
  // (which may load async) leads, falling back to the first definition.
  const [defId, setDefId] = useState("");
  const [busy, setBusy] = useState(false);

  const preferredId = defId || defaultWorkflowId || "";
  const def = definitions.find((d) => d.id === preferredId) ?? definitions[0];

  const pickDef = (id: string) => {
    setDefId(id);
    if (projectId) rememberDefaultWorkflow(projectId, id);
  };
  const steps = flattenSteps(def?.spec ?? null);

  const resolve = (alias: string) =>
    resolveAlias(def?.spec.agents, alias, customAgents, modelsByAgent);

  // Slash commands are provider-specific; a workflow has many agents, so drive
  // the `/` source off the first step's resolved provider (fallback: claude).
  const slashProvider =
    (steps[0] && resolve(steps[0].agentAlias)?.providerId) || DEFAULT_PROVIDER_ID;

  // Fire the launch via a ref so `onEnter` can reference `launch` (defined below,
  // since it reads the input's text/attachments).
  const submitRef = useRef<() => void>(() => {});
  const input = useComposerInput({
    provider: slashProvider,
    // Repo-path-keyed mention sources (no agent/checkout exists pre-launch).
    projectDir: repoPath,
    mentionSource: () => api.listRepoTree(repoPath),
    listDir: api.listDir,
    listPrs: () => api.listRepoPrs(repoPath),
    autoFocus: true,
    onEnter: () => submitRef.current(),
  });

  const canLaunch = !!def && !!input.text.trim() && !busy;

  const launch = async () => {
    if (!def || !input.text.trim() || busy) return;
    setBusy(true);
    try {
      // project_id is resolved authoritatively from repo_path by the backend
      // (wf_launch), so the launcher doesn't guess it from the workspace snapshot.
      const runId = await api.wfLaunch(
        def.spec,
        input.text.trim(),
        "",
        repoPath,
        def.id,
        baseBranch || undefined,
        input.attachments,
        undefined,
        issueRef,
      );
      selectRun(runId);
    } catch (e) {
      setLastError(`Failed to launch workflow: ${e}`);
      setBusy(false);
    }
  };
  submitRef.current = () => void launch();

  if (loading) {
    return (
      <div className="wf-field-empty" style={{ textAlign: "center" }}>
        Loading workflows…
      </div>
    );
  }

  if (definitions.length === 0) {
    return (
      <div className="wf-field-empty" style={{ textAlign: "center" }}>
        <p style={{ margin: "0 0 10px" }}>
          No pipelines yet — build one to chain agents on a task.
        </p>
        <button
          type="button"
          className="btn-t primary"
          onClick={() => openSettingsScreen("workflows")}
        >
          <Icon name="combine" size={13} /> Create a workflow
        </button>
      </div>
    );
  }

  return (
    <ComposerFrame
      input={input}
      placeholder="Describe the task for the workflow. · /commands · @ to attach · # for PRs"
      top={
        // The selected flow's step chain, built into the top of the prompt box so
        // it reads as "this task launches this flow" — not a detached strip.
        def && (
          <div className="cmp-flow-strip">
            <span className="cf-tag">
              <Icon name="combine" size={11} /> {def.name}
            </span>
            {steps.map((s, i) => {
              const a = resolve(s.agentAlias);
              return (
                <Fragment key={s.id}>
                  {i > 0 && <Icon name="arrowR" size={11} className="cf-arr" />}
                  {a ? (
                    <AgentAvatar
                      custom={a.custom}
                      slug={a.providerId}
                      short={a.short}
                      hue={a.hue}
                      size={20}
                    />
                  ) : (
                    <span className="cf-q">?</span>
                  )}
                </Fragment>
              );
            })}
            <span className="cf-note">each step runs its own agent</span>
          </div>
        )
      }
      foot={
        <>
          <WorkflowSelect definitions={definitions} selected={def} onPick={pickDef} />
          <span style={{ flex: 1 }} />
          <button
            type="button"
            className="send"
            disabled={!canLaunch}
            onClick={() => void launch()}
            aria-label="Launch run"
          >
            <Icon name={busy ? "refresh" : "arrowUp"} size={13} />
          </button>
        </>
      }
    />
  );
}

/** Workflow selector styled exactly like the composer's model-chip. */
function WorkflowSelect({
  definitions,
  selected,
  onPick,
}: {
  definitions: Definition[];
  selected: Definition | undefined;
  onPick: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const stepCount = (spec: Spec) => flattenSteps(spec).length;
  return (
    <div className="model-picker" style={{ position: "relative" }}>
      <Chip bordered tip="Workflow" className="model-chip" onClick={() => setOpen((v) => !v)}>
        <span
          className="wf-srow-tile"
          style={{ "--h": FLOW_HUE, width: 15, height: 15 } as CSSProperties}
        >
          <Icon name="combine" size={10} />
        </span>
        <span className="model-chip-agent">{selected?.name ?? "Pick a workflow"}</span>
        <span className="model-chip-model">
          {selected ? `${stepCount(selected.spec)} steps` : ""}
        </span>
        <Icon name="chevD" size={9} />
      </Chip>
      {open && (
        <>
          <div style={{ position: "fixed", inset: 0, zIndex: 55 }} onClick={() => setOpen(false)} />
          <div
            className="dd"
            style={{
              position: "absolute",
              bottom: "calc(100% + 6px)",
              left: 0,
              zIndex: 56,
              minWidth: 240,
            }}
          >
            {definitions.map((d) => (
              <div
                key={d.id}
                className={`dd-item ${d.id === selected?.id ? "active" : ""}`}
                onClick={() => {
                  onPick(d.id);
                  setOpen(false);
                }}
              >
                <span>{d.name}</span>
                <span style={{ marginLeft: "auto", color: "var(--fg-3)", fontSize: 11 }}>
                  {stepCount(d.spec)} steps
                </span>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
