import { useAppStore } from "@/store";
import { type FeatureItem, SetGroup, SetHead, SetRow, SetToggle } from "./primitives";

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

/** Which surfaces appear around an agent: the right-rail panels and the
 *  composer's optional controls. */
export function WorkspacePane() {
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
        eyebrow="Settings · Workspace"
        title="Workspace"
        desc="Choose which panels and composer controls appear while you work with an agent."
      />

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
