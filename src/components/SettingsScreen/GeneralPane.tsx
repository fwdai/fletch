import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { Select } from "@/components/ui/Select";
import { CODE_THEMES } from "@/data/codeThemes";
import { ACCENTS } from "@/data/providers";
import type { ThemeMode } from "@/storage/preferences";
import { useAppStore } from "@/store";
import { IS_MAC } from "@/util/platform";
import { DictationGroup } from "./Dictation";
import { type FeatureItem, SetGroup, SetHead, SetRow, SetSeg, SetToggle } from "./primitives";

const CODE_THEME_OPTIONS = CODE_THEMES.map((t) => ({ value: t.id, label: t.label }));

const SIDE_PANELS: FeatureItem[] = [
  { key: "git", title: "Git", sub: "Branch, changed files, and commit, push, and PR actions." },
  {
    key: "code",
    title: "Code",
    sub: "Browse and edit checkout files, with a live feed of the agent's diffs.",
  },
  {
    key: "run",
    title: "Run",
    sub: "Start the project's dev server with auto-detected, editable config.",
  },
  { key: "terminal", title: "Terminal", sub: "Interactive shell scoped to the checkout." },
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
    sub: "Show the context-window usage meter in the composer.",
  },
];

/** One toggle row per feature flag. */
function FeatureRows({ items }: { items: FeatureItem[] }) {
  const features = useAppStore((s) => s.features);
  const setFeature = useAppStore((s) => s.setFeature);
  return items.map((it) => (
    <SetRow key={it.key} title={it.title} sub={it.sub}>
      <SetToggle on={!!features[it.key]} onClick={() => setFeature(it.key, !features[it.key])} />
    </SetRow>
  ));
}

/** App-wide basics: look, which panels and composer controls appear around an
 *  agent, how you dictate to it, alerts, and diagnostics. Anything with more
 *  than a knob or two (git, sandboxing, remote control) has its own section.
 *  Group labels and order match the quick-settings popover so the same setting
 *  reads the same on both. */
export function GeneralPane() {
  const theme = useAppStore((s) => s.theme);
  const setTheme = useAppStore((s) => s.setTheme);
  const accent = useAppStore((s) => s.accent);
  const setAccent = useAppStore((s) => s.setAccent);
  const codeTheme = useAppStore((s) => s.codeTheme);
  const setCodeTheme = useAppStore((s) => s.setCodeTheme);
  const soundEnabled = useAppStore((s) => s.soundEnabled);
  const setSoundEnabled = useAppStore((s) => s.setSoundEnabled);
  const notifyEnabled = useAppStore((s) => s.notifyEnabled);
  const setNotifyEnabled = useAppStore((s) => s.setNotifyEnabled);
  const notifyTurnComplete = useAppStore((s) => s.notifyTurnComplete);
  const setNotifyTurnComplete = useAppStore((s) => s.setNotifyTurnComplete);
  const telemetryEnabled = useAppStore((s) => s.telemetryEnabled);
  const setTelemetryEnabled = useAppStore((s) => s.setTelemetryEnabled);
  const revealLogs = useAppStore((s) => s.revealLogs);

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · General"
        title="General"
        desc="How Fletch looks, which panels and composer controls appear around an agent, how you dictate to it, when it alerts you, and what it shares."
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
        <SetRow title="Code theme" sub="Syntax highlighting in the Code panel.">
          <Select
            value={codeTheme}
            ariaLabel="Code theme"
            options={CODE_THEME_OPTIONS}
            onChange={setCodeTheme}
          />
        </SetRow>
      </SetGroup>

      <SetGroup label="Side panels">
        <FeatureRows items={SIDE_PANELS} />
      </SetGroup>

      <SetGroup label="Composer">
        <FeatureRows items={COMPOSER} />
      </SetGroup>

      {/* Right under Composer: dictation is another way of talking to it.
          macOS-only, like the recognizer it replaces. */}
      {IS_MAC && <DictationGroup />}

      <SetGroup label="Notifications">
        <SetRow title="Sound" sub="Play a chime when an agent finishes or needs your input.">
          <SetToggle on={soundEnabled} onClick={() => setSoundEnabled(!soundEnabled)} />
        </SetRow>
        <SetRow
          title="Desktop notifications"
          sub="Show a system notification when an agent finishes or needs your input."
        >
          <SetToggle on={notifyEnabled} onClick={() => setNotifyEnabled(!notifyEnabled)} />
        </SetRow>
        <SetRow
          title="Turn finished"
          sub="Also alert when an agent finishes a turn. Alerts for needed input are always sent."
        >
          <SetToggle
            on={notifyTurnComplete}
            onClick={() => setNotifyTurnComplete(!notifyTurnComplete)}
          />
        </SetRow>
      </SetGroup>

      <SetGroup label="Privacy & diagnostics" last>
        <SetRow
          title="Usage analytics"
          sub="Anonymous usage events that help improve Fletch. Never includes code, paths, or prompts."
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
