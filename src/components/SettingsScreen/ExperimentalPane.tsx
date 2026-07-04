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

  // Local edit state, committed on blur/Enter so we don't persist per keystroke.
  const [image, setImage] = useState(dockerImage);
  const [memory, setMemory] = useState(dockerMemory);
  const [cpus, setCpus] = useState(dockerCpus);

  // Reflect external changes (e.g. a revert on a failed save) back into the
  // fields so they never drift from the store's source of truth.
  useEffect(() => setImage(dockerImage), [dockerImage]);
  useEffect(() => setMemory(dockerMemory), [dockerMemory]);
  useEffect(() => setCpus(dockerCpus), [dockerCpus]);

  const commit = () => {
    const [i, m, c] = [image.trim(), memory.trim(), cpus.trim()];
    if (i === dockerImage && m === dockerMemory && c === dockerCpus) return;
    void save(i, m, c);
  };

  const commitOnEnter = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") e.currentTarget.blur();
  };

  return (
    <SetGroup label="Docker sandbox" last>
      <SetRow
        title="Container image"
        sub="Override the built-in agent image. Your image must have Claude Code (`claude`) on PATH and git installed. Leave blank to use Fletch's image (built on the first Docker run)."
      >
        <input
          className="set-text text-base mono"
          value={image}
          placeholder="fletch-agent (built-in)"
          spellCheck={false}
          onChange={(e) => setImage(e.target.value)}
          onBlur={commit}
          onKeyDown={commitOnEnter}
        />
      </SetRow>
      <SetRow
        title="Memory limit"
        sub="Passed to `docker run --memory`. Blank uses the default (4g)."
      >
        <input
          className="set-text text-base mono"
          value={memory}
          placeholder="4g"
          spellCheck={false}
          onChange={(e) => setMemory(e.target.value)}
          onBlur={commit}
          onKeyDown={commitOnEnter}
        />
      </SetRow>
      <SetRow title="CPU limit" sub="Passed to `docker run --cpus`. Blank uses the default (2).">
        <input
          className="set-text text-base mono"
          value={cpus}
          placeholder="2"
          spellCheck={false}
          onChange={(e) => setCpus(e.target.value)}
          onBlur={commit}
          onKeyDown={commitOnEnter}
        />
      </SetRow>
    </SetGroup>
  );
}
