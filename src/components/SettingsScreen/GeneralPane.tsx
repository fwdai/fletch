import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { Select } from "@/components/ui/Select";
import { CODE_THEMES } from "@/data/codeThemes";
import { ACCENTS } from "@/data/providers";
import type { ThemeMode } from "@/storage/preferences";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { SetGroup, SetHead, SetRow, SetSeg, SetToggle } from "./primitives";

const CODE_THEME_OPTIONS = CODE_THEMES.map((t) => ({ value: t.id, label: t.label }));

/** The Select wants string values; the store keeps days. `0` = off. */
const AUTO_ARCHIVE_OPTIONS: { value: string; label: string }[] = [
  { value: "0", label: "Off" },
  { value: "3", label: "3 days" },
  { value: "7", label: "7 days" },
  { value: "14", label: "14 days" },
  { value: "30", label: "30 days" },
];

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
  const notifyPrActivity = useAppStore((s) => s.notifyPrActivity);
  const setNotifyPrActivity = useAppStore((s) => s.setNotifyPrActivity);
  const telemetryEnabled = useAppStore((s) => s.telemetryEnabled);
  const setTelemetryEnabled = useAppStore((s) => s.setTelemetryEnabled);
  const autoArchiveIdleDays = useAppStore((s) => s.autoArchiveIdleDays);
  const setAutoArchiveIdleDays = useAppStore((s) => s.setAutoArchiveIdleDays);
  const revealLogs = useAppStore((s) => s.revealLogs);
  // The two alert switches and the sweep are the host's (it sends the alerts
  // and runs the sweep), so they edit the environment on screen.
  const hostGate = useGate("hostSettings");

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · General"
        title="General"
        desc="How Fletch looks, when it alerts you, and what it shares."
      />

      <SetGroup label="Appearance">
        <SetRow title="Theme">
          <SetSeg<ThemeMode>
            value={theme}
            options={[
              { value: "dark", label: "Dark" },
              { value: "light", label: "Light" },
            ]}
            onChange={setTheme}
          />
        </SetRow>
        <SetRow title="Accent" sub="Highlight color across the app.">
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
        <SetRow
          title="Sound"
          sub="Play a sound when an agent finishes, needs input, or hits an error."
        >
          <SetToggle on={soundEnabled} onClick={() => setSoundEnabled(!soundEnabled)} />
        </SetRow>
        <SetRow title="Desktop notifications" sub="Show a system notification for the same events.">
          <SetToggle on={notifyEnabled} onClick={() => setNotifyEnabled(!notifyEnabled)} />
        </SetRow>
        <SetRow
          title="Turn finished"
          sub="Also alert on finished turns. Input requests and errors always alert."
        >
          <SetToggle
            on={notifyTurnComplete}
            onClick={() => setNotifyTurnComplete(!notifyTurnComplete)}
            disabled={!!hostGate}
            tip={hostGate ?? undefined}
          />
        </SetRow>
        <SetRow
          title="Pull request activity"
          sub="Alert your phone on check results, reviews, and merged or closed PRs."
        >
          <SetToggle
            on={notifyPrActivity}
            onClick={() => setNotifyPrActivity(!notifyPrActivity)}
            disabled={!!hostGate}
            tip={hostGate ?? undefined}
          />
        </SetRow>
      </SetGroup>

      <SetGroup label="Workspaces">
        <SetRow
          title="Auto-archive idle workspaces"
          sub={
            hostGate ??
            "Archive fully pushed workspaces after this long, or a day after their PR closes. Restore them from History."
          }
        >
          <Select
            value={String(autoArchiveIdleDays)}
            ariaLabel="Auto-archive idle workspaces"
            disabled={!!hostGate}
            options={AUTO_ARCHIVE_OPTIONS}
            onChange={(v) => void setAutoArchiveIdleDays(Number(v))}
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
        <SetRow title="Logs" sub="Find the local log file to attach to a bug report.">
          <Button variant="outline" onClick={() => void revealLogs()}>
            <Icon name="folder" size={12} />
            Reveal logs
          </Button>
        </SetRow>
      </SetGroup>
    </div>
  );
}
