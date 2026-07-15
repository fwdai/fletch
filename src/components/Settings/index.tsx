import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { Scrim } from "@/components/ui/Scrim";
import { PROVIDER_DETAIL } from "@/data/providerDetail";
import { ACCENTS, PROVIDERS } from "@/data/providers";
import type { FeatureFlags, ThemeMode } from "@/storage/preferences";
import { useAppStore } from "@/store";
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
      { key: "terminal", title: "Terminal", sub: "Interactive shell in the checkout" },
    ],
  },
  {
    label: "Composer",
    items: [
      { key: "thinkingBudget", title: "Thinking budget", sub: "Low / medium / high cap" },
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
  const providerVersions = useAppStore((s) => s.providerVersions);
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
        {PROVIDERS.map((p) => {
          // Honest, non-user-specific model routing, plus the live-probed
          // version when the backend has resolved it — never a fabricated plan
          // name or version string.
          const version = providerVersions[p.id];
          const models = PROVIDER_DETAIL[p.id].models;
          const description = [models, version].filter(Boolean).join(" · ");
          return (
            <SettingsRow key={p.id} label={p.label} description={description}>
              <Toggle
                value={providerFlags[p.id] !== false}
                onChange={(v) => setProviderEnabled(p.id, v)}
              />
            </SettingsRow>
          );
        })}
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
