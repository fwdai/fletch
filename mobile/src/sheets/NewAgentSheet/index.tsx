import { Icon } from "@desktop/components/Icon";
import { useCallback, useEffect, useState } from "react";
import { useAttachments } from "../../attachments";
import { PromptField } from "../../components/PromptField";
import { PickerSheet, ProviderMark, Sheet, Swatch } from "../../components/ui";
import { Notice } from "../../components/ui/Notice";
import { modelLabel, providerLabel } from "../../lib/agents";
import { hostProviderBlock, useHostProviders } from "../../lib/hostProviders";
import { ignore } from "../../lib/ignore";
import { modelsFor, useModels } from "../../lib/models";
import { primaryPath, projectsOf } from "../../lib/projects";
import { api, useStore } from "../../store";
import { RunnerSheet } from "./RunnerSheet";

type Picker = "project" | "branch" | "runner" | null;

export function NewAgentSheet({
  open,
  onClose,
  projectId,
}: {
  open: boolean;
  onClose: () => void;
  projectId?: string;
}) {
  // Derived from the snapshot, not selected: `projectsOf` is stable per
  // snapshot, so neither the render nor the reset effect below loops.
  const workspace = useStore((s) => s.workspace);
  const projects = projectsOf(workspace);
  const spawn = useStore((s) => s.spawn);
  const lastError = useStore((s) => s.lastError);
  const clearError = useStore((s) => s.clearError);
  const models = useModels();
  // Which agents the Mac can actually start, re-read each time the sheet
  // opens. Null until it answers (or forever, on a host too old for the op),
  // and null blocks nothing.
  const hostProviders = useHostProviders(open);

  const [pid, setPid] = useState(projectId ?? projects[0]?.id ?? "");
  const [prompt, setPrompt] = useState("");
  const [provider, setProvider] = useState("claude");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("high");
  const [branches, setBranches] = useState<string[]>([]);
  const [base, setBase] = useState<string | null>(null);
  const [defaultBase, setDefaultBase] = useState("main");
  const [name, setName] = useState("");
  const [picker, setPicker] = useState<Picker>(null);
  const [starting, setStarting] = useState(false);
  // A live mic holds the Start button: the words are still on their way into
  // the draft, and spawning now would send it without them. A file still
  // uploading holds it for the same reason.
  const [dictating, setDictating] = useState(false);
  const attachments = useAttachments();

  const project = projects.find((p) => p.id === pid) ?? projects[0];
  // The phone spawns in the project's primary repo (single-repo for now).
  const repoPath = project ? primaryPath(project) : null;

  const allocate = useCallback(async (taken: string[]) => {
    const next = await api.allocateDraftName(taken).catch(() => "");
    if (next) setName(next);
  }, []);

  const firstProjectId = projects[0]?.id;
  useEffect(() => {
    if (!open) return;
    setPid(projectId ?? firstProjectId ?? "");
    setBase(null);
    setStarting(false);
    clearError();
    void allocate([]);
  }, [open, projectId, firstProjectId, allocate, clearError]);

  useEffect(() => {
    if (!open || !repoPath) return;
    let live = true;
    void Promise.all([
      api.listRepoBranches(repoPath).catch(() => [] as string[]),
      api.repoDefaultBranch(repoPath).catch(() => "main"),
    ]).then(([list, fallback]) => {
      if (!live) return;
      setBranches(list);
      setDefaultBase(fallback);
    });
    return () => {
      live = false;
    };
  }, [open, repoPath]);

  if (!project || !repoPath) {
    return (
      <Sheet open={open} onClose={onClose} full title="New agent">
        <div className="empty">
          <b>No projects on the host</b>
          Add one from Home first, or pin a repo in Fletch on your Mac.
        </div>
      </Sheet>
    );
  }

  const chosenBase = base ?? defaultBase;
  const providerModels = modelsFor(models, provider);
  // Text or a file, not necessarily both: a screenshot can be the whole brief.
  const hasDraft = prompt.trim().length > 0 || attachments.paths.length > 0;
  // The chosen agent is one the host cannot start: hold the button rather than
  // let the spawn reach the host and fail there. The selection is left alone —
  // switching it for the user would start a different agent than the one they
  // picked.
  const providerBlocked = hostProviderBlock(hostProviders, provider);
  const held = starting || dictating || attachments.uploading || providerBlocked !== null;
  const start = () => {
    if (!hasDraft || held) return;
    setStarting(true);
    clearError();
    void spawn({
      repoPath,
      provider,
      model: model || null,
      effort,
      base: chosenBase,
      prompt: prompt.trim(),
      attachments: attachments.paths,
      name,
    })
      // Only a spawn that actually started the agent has consumed the prompt.
      // A failed one keeps it, so the button below can just be pressed again.
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
        title="New agent"
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
            <Icon name="play" size={15} />
            {starting ? "Starting…" : `Start ${name || "agent"}`}
          </button>
        }
      >
        <div className="na-mark">
          <span className="d" />
          NEW WORKSPACE
          <span className="in">· in</span>
          <button type="button" className="pj" onClick={() => setPicker("project")}>
            <Swatch project={project} size={14} />
            {project.name}
            <Icon name="chevD" size={11} />
          </button>
        </div>
        <h1 className="na-title">
          What should{" "}
          <button
            type="button"
            className="nm"
            onClick={() => void allocate(name ? [name] : [])}
            title="Reroll name"
          >
            {name || "…"}
          </button>{" "}
          do?
        </h1>
        <p className="na-sub">
          A worktree and branch are created on the host from{" "}
          <button type="button" className="mono lnk" onClick={() => setPicker("branch")}>
            {chosenBase}
          </button>
          .
        </p>
        <PromptField
          value={prompt}
          onChange={setPrompt}
          onDictating={setDictating}
          attachments={attachments}
        >
          <button type="button" className="chip runner" onClick={() => setPicker("runner")}>
            <ProviderMark id={provider} />
            <span>{providerLabel(provider)}</span>
            <span className="sep">·</span>
            <span>{modelLabel(model)}</span>
            <span className="sep">·</span>
            <span>{effort}</span>
            <Icon name="chevD" size={11} style={{ color: "var(--fg-3)" }} />
          </button>
        </PromptField>
        {providerBlocked && (
          <div className="chips-note">
            {providerLabel(provider)} is {providerBlocked} on the host — pick another agent, or
            install or sign it in there.
          </div>
        )}
        {lastError && (
          <Notice tone="error" className="na-err" onDismiss={clearError}>
            {lastError}
          </Notice>
        )}
        <div className="na-ctx">
          <button type="button" className="chip" onClick={() => setPicker("project")}>
            <Swatch project={project} size={14} />
            {project.name}
          </button>
          <button type="button" className="chip" onClick={() => setPicker("branch")}>
            <Icon name="branch" size={12} />
            {chosenBase}
          </button>
        </div>
      </Sheet>
      <PickerSheet
        open={picker === "project"}
        onClose={() => setPicker(null)}
        title="Project"
        items={projects.map((p) => ({
          id: p.id,
          label: p.name,
          sub: primaryPath(p),
          icon: <Swatch project={p} />,
        }))}
        value={pid}
        onChange={(id) => {
          setPid(id);
          setBase(null);
        }}
      />
      <PickerSheet
        open={picker === "branch"}
        onClose={() => setPicker(null)}
        title="Base branch"
        searchable
        items={(branches.length ? branches : [defaultBase]).map((b) => ({
          id: b,
          label: b,
          sub: b === defaultBase ? "default branch" : null,
        }))}
        value={chosenBase}
        onChange={setBase}
      />
      <RunnerSheet
        open={picker === "runner"}
        onClose={() => setPicker(null)}
        models={models}
        provider={provider}
        model={model || (providerModels[0]?.id ?? "")}
        effort={effort}
        hostProviders={hostProviders}
        setProvider={setProvider}
        setModel={setModel}
        setEffort={setEffort}
      />
    </>
  );
}
