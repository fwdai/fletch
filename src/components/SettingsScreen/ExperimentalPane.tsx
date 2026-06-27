import { useAppStore } from "../../store";
import { type FeatureItem, SetGroup, SetHead, SetRow, SetToggle } from "./primitives";

const EXPERIMENTS: FeatureItem[] = [
  {
    key: "nativeView",
    title: "Native terminal view",
    sub: "Add a Custom / Native switch to each agent so you can drive it through the provider's own terminal UI. Fidelity varies by provider.",
  },
];

/** Home for early, opt-in features that aren't ready to be on by default.
 *  Drop new flags into EXPERIMENTS as they land. */
export function ExperimentalPane() {
  const features = useAppStore((s) => s.features);
  const setFeature = useAppStore((s) => s.setFeature);

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Experimental"
        title="Experimental"
        desc="Early features we're still polishing. Expect rough edges — toggle them on to try them, off to go back to the stable path."
      />

      <SetGroup label="Early features" last>
        {EXPERIMENTS.map((it) => (
          <SetRow key={it.key} title={it.title} sub={it.sub}>
            <SetToggle
              on={!!features[it.key]}
              onClick={() => setFeature(it.key, !features[it.key])}
            />
          </SetRow>
        ))}
      </SetGroup>
    </div>
  );
}
