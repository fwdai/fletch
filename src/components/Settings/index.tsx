import { ACCENTS, PROVIDERS } from "../../data/providers";
import type { Density, FeatureFlags, ThemeMode } from "../../storage/preferences";
import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { IconButton } from "../ui/IconButton";
import { Scrim } from "../ui/Scrim";
import { Segmented } from "./Segmented";
import { SettingsRow, SettingsSection } from "./SettingsRow";
import { Toggle } from "./Toggle";

interface FeatureItem {
  key: keyof FeatureFlags;
  title: string;
  sub: string;
}

const FEATURE_GROUPS: { label: string; items: FeatureItem[] }[] = [
  {
    label: "Side panels",
    items: [
      { key: "code", title: "Code", sub: "Browse & edit files, plus a Live diff feed" },
      { key: "git", title: "Git", sub: "Branch, file changes, smart actions" },
      { key: "run", title: "Run", sub: "Dev server with setup table" },
      { key: "terminal", title: "Terminal", sub: "Interactive shell in the worktree" },
    ],
  },
  {
    label: "Composer",
    items: [
      { key: "thinkingBudget", title: "Thinking budget", sub: "Low / medium / high cap" },
      { key: "autoEdit", title: "Auto-edit", sub: "Skip approval for write tools" },
      { key: "tokenUsage", title: "Token usage", sub: "Context window % meter in the composer" },
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
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);

  return (
    <div className="settings-pop">
      <div className="sp-h flex-center text-base">
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
      </SettingsSection>

      {FEATURE_GROUPS.map((g) => (
        <SettingsSection key={g.label} title={g.label}>
          {g.items.map((it) => (
            <SettingsRow key={it.key} label={it.title} description={it.sub}>
              <Toggle value={features[it.key]} onChange={(v) => setFeature(it.key, v)} />
            </SettingsRow>
          ))}
        </SettingsSection>
      ))}

      <SettingsSection title="Providers">
        {PROVIDERS.map((p) => (
          <SettingsRow key={p.id} label={p.label} description={`${p.sub} · ${p.version}`}>
            <Toggle
              value={providerFlags[p.id] !== false}
              onChange={(v) => setProviderEnabled(p.id, v)}
            />
          </SettingsRow>
        ))}
      </SettingsSection>

      <button className="sp-allbtn flex-center" onClick={() => openSettingsScreen("general")}>
        <Icon name="settings" size={13} />
        <span>All settings</span>
        <span className="grow" />
        <Icon name="chevR" size={12} />
      </button>
    </div>
  );
}
