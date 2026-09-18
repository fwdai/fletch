import { Icon } from "@desktop/components/Icon";
import { useEffect, useState } from "react";
import { useAttachments } from "../../attachments";
import { PromptField } from "../../components/PromptField";
import { PickerSheet, Sheet, Swatch } from "../../components/ui";
import { Notice } from "../../components/ui/Notice";
import { ignore } from "../../lib/ignore";
import { useStore } from "../../store";

/** The idea you had on the way somewhere: pick the project, say the thing, and
 *  the project's Project Manager picks it up from there.
 *
 *  Deliberately thinner than the New Agent sheet — no runner and no base
 *  branch. A PM chat runs under the Project Manager preset (its own model and
 *  reasoning budget) and never edits code, so neither choice has anything to
 *  act on; the only decision left is which project the idea belongs to. */
export function NewPlanSheet({
  open,
  onClose,
  projectId,
}: {
  open: boolean;
  onClose: () => void;
  projectId?: string;
}) {
  // Derived, not selected: a selector building a fresh array re-renders forever
  // under zustand v5, and would re-fire the reset effect below with it.
  const workspace = useStore((s) => s.workspace);
  const projects = workspace?.projects ?? [];
  const startPlanningChat = useStore((s) => s.startPlanningChat);
  const lastError = useStore((s) => s.lastError);
  const clearError = useStore((s) => s.clearError);

  const [pid, setPid] = useState(projectId ?? projects[0]?.project_id ?? "");
  const [prompt, setPrompt] = useState("");
  const [picking, setPicking] = useState(false);
  const [starting, setStarting] = useState(false);
  // A live mic holds the button: the words are still on their way into the
  // draft. A file still uploading holds it for the same reason.
  const [dictating, setDictating] = useState(false);
  const attachments = useAttachments();

  const project = projects.find((p) => p.project_id === pid) ?? projects[0];

  const firstProjectId = workspace?.projects[0]?.project_id;
  useEffect(() => {
    if (!open) return;
    setPid(projectId ?? firstProjectId ?? "");
    setStarting(false);
    clearError();
  }, [open, projectId, firstProjectId, clearError]);

  if (!project) {
    return (
      <Sheet open={open} onClose={onClose} full title="Plan with PM">
        <div className="empty">
          <b>No projects on the host</b>
          Add one from Home first, or pin a repo in Fletch on your Mac.
        </div>
      </Sheet>
    );
  }

  // A screenshot of the thing that annoyed you can be the whole idea.
  const hasDraft = prompt.trim().length > 0 || attachments.paths.length > 0;
  const held = starting || dictating || attachments.uploading;
  const start = () => {
    if (!hasDraft || held) return;
    setStarting(true);
    clearError();
    void startPlanningChat({
      projectId: project.project_id,
      prompt: prompt.trim(),
      attachments: attachments.paths,
    })
      // Only a chat that actually started has consumed the idea; a failed one
      // keeps it, so the button below can simply be pressed again.
      .then(() => {
        setPrompt("");
        attachments.clear();
      }, ignore)
      .finally(() => setStarting(false));
  };

  return (
    <>
      <Sheet
        open={open}
        onClose={onClose}
        full
        title="Plan with PM"
        left={
          <button type="button" className="tbtn q" onClick={onClose}>
            Cancel
          </button>
        }
        foot={
          <button
            type="button"
            className="btn primary block"
            disabled={!hasDraft || held}
            onClick={start}
          >
            <Icon name="map" size={15} />
            {starting ? "Starting…" : "Start planning"}
          </button>
        }
      >
        <div className="na-mark">
          <span className="d" />
          NEW PLAN
          <span className="in">· in</span>
          <button type="button" className="pj" onClick={() => setPicking(true)}>
            <Swatch project={project} size={14} />
            {project.name}
            <Icon name="chevD" size={11} />
          </button>
        </div>
        <h1 className="na-title">What's the idea?</h1>
        <p className="na-sub">
          The project manager reads {project.name}, thinks it through with you, and proposes backlog
          items. It never edits code.
        </p>
        <PromptField
          value={prompt}
          onChange={setPrompt}
          onDictating={setDictating}
          attachments={attachments}
          placeholder="Say the idea in your own words — half-formed is fine."
        />
        {lastError && (
          <Notice tone="error" className="na-err" onDismiss={clearError}>
            {lastError}
          </Notice>
        )}
        <div className="na-ctx">
          <button type="button" className="chip" onClick={() => setPicking(true)}>
            <Swatch project={project} size={14} />
            {project.name}
          </button>
        </div>
      </Sheet>
      <PickerSheet
        open={picking}
        onClose={() => setPicking(false)}
        title="Project"
        items={projects.map((p) => ({
          id: p.project_id,
          label: p.name,
          sub: p.path,
          icon: <Swatch project={p} />,
        }))}
        value={pid}
        onChange={setPid}
      />
    </>
  );
}
