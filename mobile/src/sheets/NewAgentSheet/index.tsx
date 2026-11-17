import { Icon } from "@desktop/components/Icon";
import { useCallback, useEffect, useRef, useState } from "react";
import { useAttachments } from "../../attachments";
import { PromptField } from "../../components/PromptField";
import { PickerSheet, ProjectIcon, ProviderMark, Sheet } from "../../components/ui";
import { Notice } from "../../components/ui/Notice";
import { useHost } from "../../host/useHost";
import { modelLabel, providerLabel } from "../../lib/agents";
import { hostProviderBlock, useHostProviders } from "../../lib/hostProviders";
import { ignore } from "../../lib/ignore";
import { modelsFor, useModels } from "../../lib/models";
import { primaryPath } from "../../lib/projects";
import { severalHosts, useApp, useHostName, useHostStore } from "../../store";
import { ProjectPickerSheet } from "../ProjectPickerSheet";
import { useProjectChoice } from "../useProjectChoice";
import { RunnerSheet } from "./RunnerSheet";
import { RunsOn } from "./RunsOn";

type Picker = "project" | "branch" | "runner" | null;

/** Start an agent in a project of any paired host, on the host the sheet acts
 *  on (`useProjectChoice`): the picked project's speaking host, or the one
 *  picked under "Runs on". The name, branches, providers and models are that
 *  host's, and the agent is spawned there. */
export function NewAgentSheet({
  open,
  onClose,
  projectId,
}: {
  open: boolean;
  onClose: () => void;
  projectId?: string;
}) {
  const choice = useProjectChoice("newAgent", open, projectId);
  const { entries, entry, project } = choice;
  const spawn = useHostStore((s) => s.spawn);
  const lastError = useApp((s) => s.lastError);
  const clearError = useApp((s) => s.clearError);
  const { api } = useHost();
  // Which Mac the agent starts on, said once more than one is paired.
  const hostName = useHostName();
  const onHost = useApp(severalHosts) && hostName ? hostName : "the host";
  const models = useModels();
  // Which agents the Mac can actually start, re-read each time the sheet
  // opens. Null until it answers (or forever, on a host too old for the op),
  // and null blocks nothing.
  const hostProviders = useHostProviders(open);

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

  // The phone spawns in the project's primary repo (single-repo for now), as
  // the host the sheet acts on has it.
  const repoPath = project ? primaryPath(project) : null;

  // A name is drawn by the host it is for: one that lands after the sheet has
  // moved to another host is not offered there.
  const acting = useRef(api);
  acting.current = api;
  const allocate = useCallback(
    async (taken: string[]) => {
      const next = await api.allocateDraftName(taken).catch(() => "");
      if (next && acting.current === api) setName(next);
    },
    [api],
  );

  // On opening, and on moving to another host: a name and a base branch are
  // the host's, so neither carries over.
  useEffect(() => {
    if (!open) return;
    setBase(null);
    setStarting(false);
    clearError();
    void allocate([]);
  }, [open, allocate, clearError]);

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
  }, [api, open, repoPath]);

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
  // picked. A copy of the project still being made for another host holds it
  // too: the agent is to start there, not here.
  const providerBlocked = hostProviderBlock(hostProviders, provider);
  const held =
    starting ||
    dictating ||
    attachments.uploading ||
    providerBlocked !== null ||
    choice.preparing !== null;
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
            <ProjectIcon project={project} size={14} />
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
          A worktree and branch are created on {onHost} from{" "}
          <button type="button" className="mono lnk" onClick={() => setPicker("branch")}>
            {chosenBase}
          </button>
          .
        </p>
        <RunsOn choice={choice} />
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
            <ProjectIcon project={project} size={14} />
            {project.name}
          </button>
          <button type="button" className="chip" onClick={() => setPicker("branch")}>
            <Icon name="branch" size={12} />
            {chosenBase}
          </button>
        </div>
      </Sheet>
      <ProjectPickerSheet
        open={picker === "project"}
        onClose={() => setPicker(null)}
        entries={entries}
        value={entry}
        onPick={(picked) => {
          choice.pick(picked);
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
