import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { SetHead, SetGroup, SetRow } from "./primitives";

/** Dev-only settings surface. The nav entry that routes here is gated on
 *  `import.meta.env.DEV`, so this pane only ships under `bun tauri dev`. */
export function DeveloperPane() {
  const openOnboarding = useAppStore((s) => s.openOnboarding);
  const closeSettingsScreen = useAppStore((s) => s.closeSettingsScreen);

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Developer"
        title="Developer"
        desc="Tools for working on Quorum itself. Only available in development builds."
      />

      <SetGroup label="Onboarding" last>
        <SetRow
          title="Welcome tour"
          sub="Replay the cinematic onboarding — the feature tour and first-project walkthrough."
        >
          <button
            type="button"
            className="btn-t outline"
            onClick={() => {
              closeSettingsScreen();
              openOnboarding();
            }}
          >
            <Icon name="sparkle" size={12} />
            Replay tour
          </button>
        </SetRow>
      </SetGroup>
    </div>
  );
}
