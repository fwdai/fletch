import { useAppStore, type FeatureFlags, type ThemeMode, type Density } from "../../store";
import { ACCENTS, PROVIDERS } from "../../data/providers";
import { Icon } from "../Icon";
import { IconButton } from "../ui/IconButton";
import { Scrim } from "../ui/Scrim";
import { Toggle } from "./Toggle";
import { Segmented } from "./Segmented";
import { SettingsRow, SettingsSection } from "./SettingsRow";

interface FeatureItem {
  key: keyof FeatureFlags;
  title: string;
  sub: string;
}

const FEATURE_GROUPS: { label: string; items: FeatureItem[] }[] = [
  {
    label: "Side panels",
    items: [
      { key: "files",    title: "Files",    sub: "Browse & edit worktree files" },
      { key: "git",      title: "Git",      sub: "Branch, file changes, smart actions" },
      { key: "diff",     title: "Diff",     sub: "Inline diff for the selected file" },
      { key: "run",      title: "Run",      sub: "Dev server with setup table" },
      { key: "terminal", title: "Terminal", sub: "Interactive shell in the worktree" },
    ],
  },
  {
    label: "Composer",
    items: [
      { key: "thinkingBudget", title: "Thinking budget", sub: "Low / medium / high cap" },
      { key: "autoEdit",       title: "Auto-edit",       sub: "Skip approval for write tools" },
    ],
  },
  {
    label: "Status",
    items: [
      { key: "statusBar",  title: "Status bar",  sub: "Persistent bar with branch, server, tokens" },
      { key: "tokenUsage", title: "Token usage", sub: "Context window % in status bar" },
    ],
  },
];

export function Settings() {
  const open = useAppStore((s) => s.settingsOpen);
  const close = useAppStore((s) => s.toggleSettings);
  if (!open) return null;
  return (
    <>
      <Scrim onClose={() => close(false)} zIndex={250} />
      <Popover onClose={() => close(false)} />
    </>
  );
}

function Popover({ onClose }: { onClose: () => void }) {
  const features = useAppStore((s) => s.features);
  const setFeature = useAppStore((s) => s.setFeature);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const setProviderEnabled = useAppStore((s) => s.setProviderEnabled);
  const theme = useAppStore((s) => s.theme);
  const setTheme = useAppStore((s) => s.setTheme);
  const accent = useAppStore((s) => s.accent);
  const setAccent = useAppStore((s) => s.setAccent);
  const density = useAppStore((s) => s.density);
  const setDensity = useAppStore((s) => s.setDensity);
  const showLandmarks = useAppStore((s) => s.showLandmarks);
  const setShowLandmarks = useAppStore((s) => s.setShowLandmarks);

  return (
    <div className="settings-pop">
      <div className="sp-h">
        <span>Settings</span>
        <span className="grow" />
        <IconButton size="sm" onClick={onClose} aria-label="Close">
          <Icon name="close" />
        </IconButton>
      </div>

      <SettingsSection title="Appearance">
        <SettingsRow label="Theme">
          <Segmented<ThemeMode>
            value={theme}
            options={[
              { value: "dark", label: "Dark" },
              { value: "light", label: "Light" },
            ]}
            onChange={setTheme}
          />
        </SettingsRow>
        <SettingsRow label="Accent">
          <div className="sp-swatches">
            {ACCENTS.map((a) => (
              <button
                key={a.id}
                className={`sp-swatch ${a.id === accent ? "active" : ""}`}
                style={{ background: a.color }}
                title={a.label}
                onClick={() => setAccent(a.id)}
                aria-label={a.label}
              />
            ))}
          </div>
        </SettingsRow>
        <SettingsRow label="Density">
          <Segmented<Density>
            value={density}
            options={[
              { value: "comfortable", label: "Comfortable" },
              { value: "compact", label: "Compact" },
            ]}
            onChange={setDensity}
          />
        </SettingsRow>
        <SettingsRow
          label="Landmark glyphs"
          description="Tiny location marks beside agent names"
        >
          <Toggle value={showLandmarks} onChange={setShowLandmarks} />
        </SettingsRow>
      </SettingsSection>

      {FEATURE_GROUPS.map((g) => (
        <SettingsSection key={g.label} title={g.label}>
          {g.items.map((it) => (
            <SettingsRow key={it.key} label={it.title} description={it.sub}>
              <Toggle
                value={features[it.key]}
                onChange={(v) => setFeature(it.key, v)}
              />
            </SettingsRow>
          ))}
        </SettingsSection>
      ))}

      <SettingsSection title="Providers">
        {PROVIDERS.map((p) => (
          <SettingsRow
            key={p.id}
            label={p.label}
            description={`${p.sub} · ${p.version}`}
          >
            <Toggle
              value={providerFlags[p.id] !== false}
              onChange={(v) => setProviderEnabled(p.id, v)}
            />
          </SettingsRow>
        ))}
      </SettingsSection>
    </div>
  );
}
