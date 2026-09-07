import { useCallback, useEffect, useRef, useState } from "react";
import { Icon } from "../../components/Icon";
import { PickerSheet, ProviderMark, Sheet, Swatch } from "../../components/ui";
import { modelLabel, providerLabel } from "../../lib/agents";
import { autosize } from "../../lib/autosize";
import { ignore } from "../../lib/ignore";
import { modelsFor, useModels } from "../../lib/models";
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
  // The projects list is derived, not selected: a selector returning a fresh
  // array is a new reference every render, which would both re-render forever
  // and re-fire the reset effect below.
  const workspace = useStore((s) => s.workspace);
  const projects = workspace?.projects ?? [];
  const spawn = useStore((s) => s.spawn);
  const lastError = useStore((s) => s.lastError);
  const clearError = useStore((s) => s.clearError);
  const models = useModels();

  const [pid, setPid] = useState(projectId ?? projects[0]?.project_id ?? "");
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
  const ta = useRef<HTMLTextAreaElement>(null);

  const project = projects.find((p) => p.project_id === pid) ?? projects[0];

  const allocate = useCallback(async (taken: string[]) => {
    const next = await api.allocateDraftName(taken).catch(() => "");
    if (next) setName(next);
  }, []);

  const firstProjectId = workspace?.projects[0]?.project_id;
  useEffect(() => {
    if (!open) return;
    setPid(projectId ?? firstProjectId ?? "");
    setBase(null);
    setStarting(false);
    clearError();
    void allocate([]);
  }, [open, projectId, firstProjectId, allocate, clearError]);

  useEffect(() => {
    if (!open || !project) return;
    let live = true;
    void Promise.all([
      api.listRepoBranches(project.path).catch(() => [] as string[]),
      api.repoDefaultBranch(project.path).catch(() => "main"),
    ]).then(([list, fallback]) => {
      if (!live) return;
      setBranches(list);
      setDefaultBase(fallback);
    });
    return () => {
      live = false;
    };
  }, [open, project]);

  if (!project) {
    return (
      <Sheet open={open} onClose={onClose} full title="New agent">
        <div className="empty">
          <b>No projects on the host</b>
          Pin a repo in Fletch on your Mac first.
        </div>
      </Sheet>
    );
  }

  const chosenBase = base ?? defaultBase;
  const providerModels = modelsFor(models, provider);
  const start = () => {
    if (!prompt.trim() || starting) return;
    setStarting(true);
    clearError();
    void spawn({
      repoPath: project.path,
      provider,
      model: model || null,
      effort,
      base: chosenBase,
      prompt: prompt.trim(),
    })
      // Only a spawn that actually started the agent has consumed the prompt.
      // A failed one keeps it, so the button below can just be pressed again.
      .then(() => setPrompt(""), ignore)
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
            disabled={!prompt.trim() || starting}
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
        <div className="na-prompt">
          <textarea
            ref={ta}
            rows={4}
            placeholder="Describe the task — this becomes the agent's first message."
            value={prompt}
            onChange={(e) => {
              setPrompt(e.target.value);
              autosize(e.currentTarget, 120, 260);
            }}
          />
          <div className="na-tools">
            <button type="button" className="chip runner" onClick={() => setPicker("runner")}>
              <ProviderMark id={provider} />
              <span>{providerLabel(provider)}</span>
              <span className="sep">·</span>
              <span>{modelLabel(model)}</span>
              <span className="sep">·</span>
              <span>{effort}</span>
              <Icon name="chevD" size={11} style={{ color: "var(--fg-3)" }} />
            </button>
            <span className="grow" />
          </div>
        </div>
        {lastError && <div className="err na-err">{lastError}</div>}
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
          id: p.project_id,
          label: p.name,
          sub: p.path,
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
        setProvider={setProvider}
        setModel={setModel}
        setEffort={setEffort}
      />
    </>
  );
}
