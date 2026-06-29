// run/WorkflowComposer.tsx — the "Workflow" composer block. It renders ONLY the
// prompt box (the host's new-agent page provides the identity, headings, and
// project/branch pickers around it, identical to the agent screen). It reuses
// the agent composer's shell classes (.composer / .composer-input /
// .composer-foot / .model-chip / .send) so the two look the same.

import { type CSSProperties, Fragment, useRef, useState } from "react";
import { Icon } from "../../components/Icon";
import { Chip } from "../../components/ui/Chip";
import { useAppStore } from "../../store";
import { AgentAvatar } from "../builder/AgentAvatar";
import { resolveAgent } from "../shared";
import type { Workflow } from "../storage";
import { launchRun } from "./engine";
import { useWorkflows } from "./useWorkflows";

const FLOW_HUE = 285;

/** Context the host's new-agent page hands to the workflow composer/heading. */
export interface ComposerContext {
  repoPath: string;
  baseBranch: string;
  name: string;
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

export function WorkflowComposer({ repoPath, baseBranch }: ComposerContext) {
  const selectRun = useAppStore((s) => s.selectRun);
  const setLastError = useAppStore((s) => s.setLastError);
  const customAgents = useAppStore((s) => s.customAgents);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);

  const workflows = useWorkflows();
  const [wfId, setWfId] = useState("");
  const [task, setTask] = useState("");
  const [busy, setBusy] = useState(false);
  const ta = useRef<HTMLTextAreaElement>(null);

  // Default to the first workflow once they load.
  const wf = workflows.find((w) => w.id === wfId) ?? workflows[0];
  const canLaunch = !!wf && !!task.trim() && !busy;

  const grow = (el: HTMLTextAreaElement) => {
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 240)}px`;
  };

  const launch = async () => {
    if (!wf || !task.trim() || busy) return;
    setBusy(true);
    try {
      const run = await launchRun(wf, { task: task.trim(), projectId: "", repoPath, baseBranch });
      selectRun(run.id);
    } catch (e) {
      setLastError(`Failed to launch workflow: ${e}`);
      setBusy(false);
    }
  };

  if (workflows.length === 0) {
    return (
      <div className="wf-field-empty" style={{ textAlign: "center" }}>
        No workflows yet — create one in <b>Settings → Workflows</b>.
      </div>
    );
  }

  return (
    <div className="composer">
      {/* The selected flow's step chain, built into the top of the prompt box so
          it reads as "this task launches this flow" — not a detached strip. */}
      {wf && (
        <div className="cmp-flow-strip">
          <span className="cf-tag">
            <Icon name="combine" size={11} /> {wf.name}
          </span>
          {wf.steps.map((s, i) => {
            const a = resolveAgent(s.agent, customAgents, modelsByAgent);
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
      )}
      <textarea
        ref={ta}
        className="composer-input"
        rows={1}
        autoFocus
        placeholder="Describe the task for the workflow. ↵ to launch."
        value={task}
        onChange={(e) => {
          setTask(e.target.value);
          grow(e.target);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            void launch();
          }
        }}
      />
      <div className="composer-foot">
        <WorkflowSelect workflows={workflows} selected={wf} onPick={setWfId} />
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
      </div>
    </div>
  );
}

/** Workflow selector styled exactly like the composer's model-chip. */
function WorkflowSelect({
  workflows,
  selected,
  onPick,
}: {
  workflows: Workflow[];
  selected: Workflow | undefined;
  onPick: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
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
        <span className="model-chip-model">{selected ? `${selected.steps.length} steps` : ""}</span>
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
            {workflows.map((w) => (
              <div
                key={w.id}
                className={`dd-item ${w.id === selected?.id ? "active" : ""}`}
                onClick={() => {
                  onPick(w.id);
                  setOpen(false);
                }}
              >
                <span>{w.name}</span>
                <span style={{ marginLeft: "auto", color: "var(--fg-3)", fontSize: 11 }}>
                  {w.steps.length} steps
                </span>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
