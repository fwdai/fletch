import { useAppStore } from "@/store";
import { type FeatureItem, SetGroup, SetHead, SetRow, SetToggle } from "./primitives";

const SIDE_PANELS: FeatureItem[] = [
  { key: "git", title: "Git", sub: "Branch, changed files, and commit, push, and PR actions." },
  {
    key: "code",
    title: "Code",
    sub: "Browse and edit files, with a live feed of the agent's diffs.",
  },
  {
    key: "run",
    title: "Run",
    sub: "Start the project's dev server.",
  },
  { key: "terminal", title: "Terminal", sub: "Interactive shell scoped to the checkout." },
];

const HEADER: FeatureItem[] = [
  {
    key: "tokenUsage",
    title: "Token usage",
    sub: "Show how much of the context window is used, with a way to compact it.",
  },
];

const COMPOSER: FeatureItem[] = [
  {
    key: "thinkingBudget",
    title: "Thinking budget",
    sub: "Show a low / medium / high reasoning cap.",
  },
];

const SIDEBAR: FeatureItem[] = [
  {
    key: "sidebarActiveFirst",
    title: "Active projects first",
    sub: "Keep projects with running or recent agents at the top.",
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

/** Settings › Interface › Layout: what appears where around an agent — the
 *  right-rail panels, the header's context badge, the composer's optional
 *  controls, and the sidebar's extras. Group labels and order match the quick-settings popover so the same
 *  toggle reads the same on both. */
export function LayoutPane() {
  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Layout"
        title="Layout"
        desc="What appears around an agent while you work."
      />

      <SetGroup label="Side panels">
        <FeatureRows items={SIDE_PANELS} />
      </SetGroup>

      <SetGroup label="Header">
        <FeatureRows items={HEADER} />
      </SetGroup>

      <SetGroup label="Composer">
        <FeatureRows items={COMPOSER} />
      </SetGroup>

      <SetGroup label="Sidebar" last>
        <FeatureRows items={SIDEBAR} />
      </SetGroup>
    </div>
  );
}
