import { useAppStore, type FeatureFlags, type ThemeMode, type Density } from "../../store";
import { ACCENTS } from "../../data/providers";
import { Icon } from "../Icon";
import { SetHead, SetGroup, SetRow, SetToggle, SetSeg } from "./primitives";

interface FeatureItem {
  key: keyof FeatureFlags;
  title: string;
  sub: string;
}

const SIDE_PANELS: FeatureItem[] = [
  { key: "git",      title: "Git",      sub: "Branch, file changes, and smart commit / push / PR actions." },
  { key: "code",     title: "Code",     sub: "Browse & edit worktree files, plus a Live feed of the agent's diffs." },
  { key: "run",      title: "Run",      sub: "Dev server with an auto-detected, overrideable config." },
  { key: "terminal", title: "Terminal", sub: "Interactive shell scoped to the worktree." },
];

const COMPOSER: FeatureItem[] = [
  { key: "thinkingBudget", title: "Thinking budget", sub: "Show a low / medium / high reasoning cap in the composer." },
  { key: "autoEdit",       title: "Auto-edit",       sub: "Let agents apply write tools without per-tool approval." },
  { key: "tokenUsage",     title: "Token usage",     sub: "Show the context-window % meter in the composer." },
];

export function GeneralPane() {
  const theme = useAppStore((s) => s.theme);
  const setTheme = useAppStore((s) => s.setTheme);
  const accent = useAppStore((s) => s.accent);
  const setAccent = useAppStore((s) => s.setAccent);
  const density = useAppStore((s) => s.density);
  const setDensity = useAppStore((s) => s.setDensity);
  const showLandmarks = useAppStore((s) => s.showLandmarks);
  const setShowLandmarks = useAppStore((s) => s.setShowLandmarks);
  const features = useAppStore((s) => s.features);
  const setFeature = useAppStore((s) => s.setFeature);

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
        desc="Tune how Quorum looks and which surfaces appear while you work. Changes apply instantly across every agent."
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
                className={`set-swatch ${a.id === accent ? "active" : ""}`}
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
        <SetRow title="Landmark glyphs" sub="Tiny location marks beside each agent name.">
          <SetToggle on={showLandmarks} onClick={() => setShowLandmarks(!showLandmarks)} />
        </SetRow>
      </SetGroup>

      <SetGroup label="Side panels">
        {SIDE_PANELS.map((it) => (
          <FeatureRow key={it.key} item={it} />
        ))}
      </SetGroup>

      <SetGroup label="Composer" last>
        {COMPOSER.map((it) => (
          <FeatureRow key={it.key} item={it} />
        ))}
      </SetGroup>
    </div>
  );
}
