import { useState } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { SetGroup, SetHead, SetRow, SetToggle } from "./primitives";
import { BoxKnob, IMAGE_URL_FIELD } from "./Sandbox/BoxKnobs";

/** Dev-only settings surface. The nav entry that routes here is gated on
 *  `import.meta.env.DEV`, so this pane only ships under `bun tauri dev`. */
export function DeveloperPane() {
  const openOnboarding = useAppStore((s) => s.openOnboarding);
  const openFeedbackNudge = useAppStore((s) => s.openFeedbackNudge);
  const closeSettingsScreen = useAppStore((s) => s.closeSettingsScreen);
  const refreshModelCatalog = useAppStore((s) => s.refreshModelCatalog);
  const setUpdateReady = useAppStore((s) => s.setUpdateReady);
  const features = useAppStore((s) => s.features);
  const setFeature = useAppStore((s) => s.setFeature);
  const contextLayerEnabled = useAppStore((s) => s.contextLayerEnabled);
  const setContextLayerEnabled = useAppStore((s) => s.setContextLayerEnabled);
  const boxImageUrl = useAppStore((s) => s.boxImageUrl);
  const [refreshingModels, setRefreshingModels] = useState(false);

  const handleRefreshModels = async () => {
    if (refreshingModels) return;
    setRefreshingModels(true);
    try {
      await refreshModelCatalog(true);
    } finally {
      setRefreshingModels(false);
    }
  };

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Developer"
        title="Developer"
        desc="Tools for working on Fletch itself."
      />

      <SetGroup label="Home">
        <SetRow
          title="Mission Control"
          sub="Use the fleet review queue as Home, instead of the quick-actions screen."
        >
          <SetToggle
            on={!!features.missionControl}
            onClick={() => setFeature("missionControl", !features.missionControl)}
          />
        </SetRow>
      </SetGroup>

      <SetGroup label="Pilots">
        <SetRow
          title="Project context layer"
          sub="Agents read and record project knowledge. Adds a Context tab to every project page."
        >
          <SetToggle
            on={contextLayerEnabled}
            onClick={() => setContextLayerEnabled(!contextLayerEnabled)}
          />
        </SetRow>
      </SetGroup>

      <SetGroup label="Sandbox">
        <BoxKnob field={IMAGE_URL_FIELD} value={boxImageUrl} />
      </SetGroup>

      <SetGroup label="Onboarding">
        <SetRow title="Onboarding" sub="Replay the first-run welcome and pre-flight check.">
          <Button
            variant="outline"
            onClick={() => {
              closeSettingsScreen();
              openOnboarding();
            }}
          >
            <Icon name="sparkle" size={12} />
            Replay tour
          </Button>
        </SetRow>
      </SetGroup>

      <SetGroup label="Models">
        <SetRow
          title="Refresh models"
          sub="Clear the cached catalog and re-run Codex discovery plus models.dev enrichment right now."
        >
          <Button
            variant="outline"
            onClick={() => void handleRefreshModels()}
            disabled={refreshingModels}
          >
            <Icon name="refresh" size={12} />
            {refreshingModels ? "Refreshing..." : "Refresh models"}
          </Button>
        </SetRow>
      </SetGroup>

      <SetGroup label="Notifications" last>
        <SetRow
          title="Feedback prompt"
          sub="Show the “How's Fletch working for you?” prompt. Its schedule stays untouched."
        >
          <Button
            variant="outline"
            onClick={() => {
              closeSettingsScreen();
              openFeedbackNudge();
            }}
          >
            <Icon name="megaphone" size={12} />
            Trigger prompt
          </Button>
        </SetRow>
        <SetRow
          title="Update-ready toast"
          sub="Show the “Update ready” toast with a fake version. Restart now really relaunches."
        >
          <Button
            variant="outline"
            onClick={() => {
              closeSettingsScreen();
              setUpdateReady(
                "9.9.9-test",
                "• Faster startup\n• Fixed a crash when opening large diffs\n• Polished the update toast",
              );
            }}
          >
            <Icon name="sparkle" size={12} />
            Trigger toast
          </Button>
        </SetRow>
      </SetGroup>
    </div>
  );
}
