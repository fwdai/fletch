import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { Select } from "@/components/ui/Select";
import { CODE_THEMES } from "@/data/codeThemes";
import { ACCENTS } from "@/data/providers";
import type { ThemeMode } from "@/storage/preferences";
import { useAppStore } from "@/store";
import { SetGroup, SetHead, SetRow, SetSeg, SetToggle } from "./primitives";

const CODE_THEME_OPTIONS = CODE_THEMES.map((t) => ({ value: t.id, label: t.label }));

/** App-wide basics only. Anything scoped to a feature (workspace panels, git,
 *  sandboxing, dictation) has its own section. */
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
        desc="How Fletch looks, when it alerts you, and what it shares."
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
