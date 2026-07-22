import { useEffect, useState } from "react";
import { useAppStore } from "@/store";
import { type FeatureItem, SetGroup, SetHead, SetRow, SetToggle } from "./primitives";

const EXPERIMENTS: FeatureItem[] = [
  {
    key: "nativeView",
    title: "Native terminal view",
    sub: "Add a Custom / Native switch to each agent so you can drive it through the provider's own terminal UI. Fidelity varies by provider.",
  },
];

/** Home for early, opt-in features that aren't ready to be on by default.
 *  Drop new flags into EXPERIMENTS as they land. */
export function ExperimentalPane() {
  const features = useAppStore((s) => s.features);
  const setFeature = useAppStore((s) => s.setFeature);

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Experimental"
        title="Experimental"
        desc="Early features we're still polishing. Expect rough edges — toggle them on to try them, off to go back to the stable path."
      />

      <SetGroup label="Early features">
        {EXPERIMENTS.map((it) => (
          <SetRow key={it.key} title={it.title} sub={it.sub}>
            <SetToggle
              on={!!features[it.key]}
              onClick={() => setFeature(it.key, !features[it.key])}
            />
          </SetRow>
        ))}
      </SetGroup>

      <DockerAdvanced />
    </div>
  );
}

/** The three Docker launch knobs, in display order. Each maps to a
 *  backend-owned `docker_*` setting; `key` also indexes the draft/store state. */
const DOCKER_FIELDS = [
  {
    key: "image",
    title: "Container image",
    sub: "Override the built-in agent image. It must have Claude Code (`claude`) and git on PATH. Blank uses Fletch's image.",
    placeholder: "fletch-agent (built-in)",
  },
  {
    key: "memory",
    title: "Memory limit",
    sub: "Passed to `docker run --memory`. Blank uses the default (4g).",
    placeholder: "4g",
  },
  {
    key: "cpus",
    title: "CPU limit",
    sub: "Passed to `docker run --cpus`. Blank uses the default (2).",
    placeholder: "2",
  },
] as const;

/** Advanced Docker-sandbox launch knobs. These persist to the backend-owned
 *  `docker_image` / `docker_memory` / `docker_cpus` settings AND update the
 *  in-process spawn-path mirror, so a change applies to the next docker spawn
 *  without a restart. Only relevant when the Docker engine is selected
 *  (Settings › General › Sandbox); harmless otherwise. */
function DockerAdvanced() {
  const dockerImage = useAppStore((s) => s.dockerImage);
  const dockerMemory = useAppStore((s) => s.dockerMemory);
  const dockerCpus = useAppStore((s) => s.dockerCpus);
  const save = useAppStore((s) => s.saveDockerLaunchSettings);

  const stored = { image: dockerImage, memory: dockerMemory, cpus: dockerCpus };
  // Local edit state, committed on blur/Enter so we don't persist per keystroke.
  const [draft, setDraft] = useState(stored);

  // Reflect external changes (e.g. a revert on a failed save) back into the
  // fields so they never drift from the store's source of truth. The three
  // values only ever move together (via `save`), so one effect covers them.
  useEffect(() => {
    setDraft({ image: dockerImage, memory: dockerMemory, cpus: dockerCpus });
  }, [dockerImage, dockerMemory, dockerCpus]);

  const commit = () => {
    const i = draft.image.trim();
    const m = draft.memory.trim();
    const c = draft.cpus.trim();
    if (i === dockerImage && m === dockerMemory && c === dockerCpus) return;
    void save(i, m, c);
  };

  const commitOnEnter = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") e.currentTarget.blur();
  };

  return (
    <SetGroup label="Docker sandbox" last>
      {DOCKER_FIELDS.map((f) => (
        <SetRow key={f.key} title={f.title} sub={f.sub}>
          <input
            className="set-text text-base mono"
            value={draft[f.key]}
            placeholder={f.placeholder}
            spellCheck={false}
            onChange={(e) => setDraft((d) => ({ ...d, [f.key]: e.target.value }))}
            onBlur={commit}
            onKeyDown={commitOnEnter}
          />
        </SetRow>
      ))}
    </SetGroup>
  );
}
