import { useEffect } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { Select } from "@/components/ui/Select";
import { ACCENTS } from "@/data/providers";
import type { Density, SandboxEngine, ThemeMode } from "@/storage/preferences";
import { useAppStore } from "@/store";
import { ContainerAuth } from "./ContainerAuth";
import { type FeatureItem, SetGroup, SetHead, SetRow, SetSeg, SetToggle } from "./primitives";

const SIDE_PANELS: FeatureItem[] = [
  { key: "git", title: "Git", sub: "Branch, file changes, and smart commit / push / PR actions." },
  {
    key: "code",
    title: "Code",
    sub: "Browse & edit worktree files, plus a Live feed of the agent's diffs.",
  },
  { key: "run", title: "Run", sub: "Dev server with an auto-detected, overrideable config." },
  { key: "terminal", title: "Terminal", sub: "Interactive shell scoped to the worktree." },
];

const COMPOSER: FeatureItem[] = [
  {
    key: "thinkingBudget",
    title: "Thinking budget",
    sub: "Show a low / medium / high reasoning cap in the composer.",
  },
  {
    key: "tokenUsage",
    title: "Token usage",
    sub: "Show the context-window % meter in the composer.",
  },
];

export function GeneralPane() {
  const theme = useAppStore((s) => s.theme);
  const setTheme = useAppStore((s) => s.setTheme);
  const accent = useAppStore((s) => s.accent);
  const setAccent = useAppStore((s) => s.setAccent);
  const density = useAppStore((s) => s.density);
  const setDensity = useAppStore((s) => s.setDensity);
  const features = useAppStore((s) => s.features);
  const setFeature = useAppStore((s) => s.setFeature);
  const soundEnabled = useAppStore((s) => s.soundEnabled);
  const setSoundEnabled = useAppStore((s) => s.setSoundEnabled);
  const notifyEnabled = useAppStore((s) => s.notifyEnabled);
  const setNotifyEnabled = useAppStore((s) => s.setNotifyEnabled);
  const telemetryEnabled = useAppStore((s) => s.telemetryEnabled);
  const setTelemetryEnabled = useAppStore((s) => s.setTelemetryEnabled);
  const revealLogs = useAppStore((s) => s.revealLogs);
  const sandboxEngine = useAppStore((s) => s.sandboxEngine);
  const setSandboxEngine = useAppStore((s) => s.setSandboxEngine);
  const dockerProbe = useAppStore((s) => s.dockerProbe);
  const refreshDockerProbe = useAppStore((s) => s.refreshDockerProbe);

  // Probe Docker on open — and again whenever the window regains focus — so the
  // engine option reflects the live daemon state, including when the user starts
  // Docker Desktop (as the disabled-option hint suggests) while the pane is open.
  useEffect(() => {
    void refreshDockerProbe();
    const onFocus = () => void refreshDockerProbe();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refreshDockerProbe]);

  // Three states for the docker option: enabled when the daemon answered the
  // probe; otherwise disabled with a hint saying how to fix it. `null` (probe
  // still in flight) gates off too — never offer an engine we can't confirm.
  const dockerAvailable = dockerProbe?.status === "available";
  const dockerHint = dockerAvailable
    ? dockerProbe?.version && `v${dockerProbe.version}`
    : dockerProbe?.status === "daemon-down"
      ? "Start Docker Desktop"
      : "Install Docker Desktop";

  const FeatureRow = ({ item }: { item: FeatureItem }) => (
    <SetRow title={item.title} sub={item.sub}>
      <SetToggle
        on={!!features[item.key]}
        onClick={() => setFeature(item.key, !features[item.key])}
      />
    </SetRow>
  );

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · General"
        title="General"
        desc="Tune how Fletch looks and which surfaces appear while you work. Changes apply instantly across every agent."
      />

      <SetGroup label="Appearance">
        <SetRow title="Theme" sub="Light or dark window chrome.">
          <SetSeg<ThemeMode>
            value={theme}
            options={[
              { value: "dark", label: "Dark" },
              { value: "light", label: "Light" },
            ]}
            onChange={setTheme}
          />
        </SetRow>
        <SetRow title="Accent" sub="Used for highlights, focus rings, and the running pearl.">
          <div className="set-swatches">
            {ACCENTS.map((a) => (
              <button
                key={a.id}
                type="button"
                className={`set-swatch iflex-center ${a.id === accent ? "active" : ""}`}
                style={{ ["--sw" as string]: a.color }}
                title={a.label}
                aria-label={a.label}
                onClick={() => setAccent(a.id)}
              >
                {a.id === accent && <Icon name="check" size={11} />}
              </button>
            ))}
          </div>
        </SetRow>
        <SetRow title="Density" sub="Compact tightens row heights across panels.">
          <SetSeg<Density>
            value={density}
            options={[
              { value: "comfortable", label: "Comfortable" },
              { value: "compact", label: "Compact" },
            ]}
            onChange={setDensity}
          />
        </SetRow>
      </SetGroup>

      <SetGroup label="Side panels">
        {SIDE_PANELS.map((it) => (
          <FeatureRow key={it.key} item={it} />
        ))}
      </SetGroup>

      <SetGroup label="Composer">
        {COMPOSER.map((it) => (
          <FeatureRow key={it.key} item={it} />
        ))}
      </SetGroup>

      <SetGroup label="Notifications">
        <SetRow
          title="Sound"
          sub="Play a chime when an agent finishes a turn or needs your input while you're looking elsewhere."
        >
          <SetToggle on={soundEnabled} onClick={() => setSoundEnabled(!soundEnabled)} />
        </SetRow>
        <SetRow
          title="Native notifications"
          sub="Show a desktop notification when an agent finishes a turn or needs your input while you're looking elsewhere."
        >
          <SetToggle on={notifyEnabled} onClick={() => setNotifyEnabled(!notifyEnabled)} />
        </SetRow>
      </SetGroup>

      <SetGroup label="Sandbox">
        <SetRow
          title="Engine"
          sub="Applies to newly created agents; existing agents keep the engine they started with. Docker agents run in a Linux container: builds and tests run on Linux, not macOS. Only Claude Code is available in containers for now."
        >
          <Select<SandboxEngine>
            value={sandboxEngine}
            ariaLabel="Sandbox engine"
            options={[
              { value: "sandbox-exec", label: "Seatbelt (sandbox-exec)" },
              {
                value: "docker",
                label: "Docker",
                hint: dockerHint || undefined,
                disabled: !dockerAvailable,
              },
            ]}
            onChange={(v) => void setSandboxEngine(v)}
          />
        </SetRow>
        <ContainerAuth />
      </SetGroup>

      <SetGroup label="Diagnostics" last>
        <SetRow
          title="Usage analytics"
          sub="Share anonymous usage events (app opens, agents spawned, turns completed, PRs opened) to help improve Fletch. No code, file paths, repo names, or prompts are ever sent."
        >
          <SetToggle on={telemetryEnabled} onClick={() => setTelemetryEnabled(!telemetryEnabled)} />
        </SetRow>
        <SetRow
          title="Logs"
          sub="Fletch writes a local log file. Reveal it to attach to a bug report."
        >
          <Button variant="outline" onClick={() => void revealLogs()}>
            <Icon name="folder" size={12} />
            Reveal logs
          </Button>
        </SetRow>
      </SetGroup>
    </div>
  );
}
