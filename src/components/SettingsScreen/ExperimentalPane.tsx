import { useEffect, useState } from "react";
import { TextInput } from "@/components/ui/TextInput";
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

      <ContainerLaunchKnobs runtime="docker" />
      <ContainerLaunchKnobs runtime="podman" last />
    </div>
  );
}

/** The two container runtimes that carry launch knobs, and the only copy that
 *  differs between them: the group heading and the `run` command name. */
const RUNTIMES = {
  docker: { label: "Docker", bin: "docker" },
  podman: { label: "Podman", bin: "podman" },
} as const;

type Runtime = keyof typeof RUNTIMES;

/** The three launch knobs, in display order. Each maps to a backend-owned
 *  `<runtime>_*` setting; `key` also indexes the draft/store state. */
const LAUNCH_FIELDS = [
  {
    key: "image",
    title: "Container image",
    sub: (_: Runtime) =>
      "Override the built-in agent image. It must have Claude Code (`claude`) and git on PATH. Blank uses Fletch's image.",
    placeholder: "fletch-agent (built-in)",
  },
  {
    key: "memory",
    title: "Memory limit",
    sub: (r: Runtime) =>
      `Passed to \`${RUNTIMES[r].bin} run --memory\`. Blank uses the default (4g).`,
    placeholder: "4g",
  },
  {
    key: "cpus",
    title: "CPU limit",
    sub: (r: Runtime) => `Passed to \`${RUNTIMES[r].bin} run --cpus\`. Blank uses the default (2).`,
    placeholder: "2",
  },
] as const;

/** Advanced container-sandbox launch knobs for one runtime. These persist to the
 *  backend-owned `<runtime>_image` / `_memory` / `_cpus` settings AND update the
 *  in-process spawn-path mirror, so a change applies to the next spawn under
 *  that runtime without a restart. Only relevant when that engine is selected
 *  (Settings › General › Sandbox); harmless otherwise. */
function ContainerLaunchKnobs({ runtime, last }: { runtime: Runtime; last?: boolean }) {
  // One selector per value: a selector returning a fresh object would hand
  // `useSyncExternalStore` a new snapshot on every render.
  const image = useAppStore((s) => (runtime === "docker" ? s.dockerImage : s.podmanImage));
  const memory = useAppStore((s) => (runtime === "docker" ? s.dockerMemory : s.podmanMemory));
  const cpus = useAppStore((s) => (runtime === "docker" ? s.dockerCpus : s.podmanCpus));
  const save = useAppStore((s) =>
    runtime === "docker" ? s.saveDockerLaunchSettings : s.savePodmanLaunchSettings,
  );

  // Local edit state, committed on blur/Enter so we don't persist per keystroke.
  const [draft, setDraft] = useState({ image, memory, cpus });

  // Reflect external changes (e.g. a revert on a failed save) back into the
  // fields so they never drift from the store's source of truth. The three
  // values only ever move together (via `save`), so one effect covers them.
  useEffect(() => {
    setDraft({ image, memory, cpus });
  }, [image, memory, cpus]);

  const commit = () => {
    const i = draft.image.trim();
    const m = draft.memory.trim();
    const c = draft.cpus.trim();
    if (i === image && m === memory && c === cpus) return;
    void save(i, m, c);
  };

  const commitOnEnter = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") e.currentTarget.blur();
  };

  return (
    <SetGroup label={`${RUNTIMES[runtime].label} sandbox`} last={last}>
      {LAUNCH_FIELDS.map((f) => (
        <SetRow key={f.key} title={f.title} sub={f.sub(runtime)}>
          <TextInput
            mono
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
