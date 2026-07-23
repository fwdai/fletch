import { useState } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { SetGroup, SetHead, SetRow, SetToggle } from "./primitives";

/** Dev-only settings surface. The nav entry that routes here is gated on
 *  `import.meta.env.DEV`, so this pane only ships under `bun tauri dev`. */
export function DeveloperPane() {
  const openOnboarding = useAppStore((s) => s.openOnboarding);
  const closeSettingsScreen = useAppStore((s) => s.closeSettingsScreen);
  const refreshModelCatalog = useAppStore((s) => s.refreshModelCatalog);
  const setUpdateReady = useAppStore((s) => s.setUpdateReady);
  const features = useAppStore((s) => s.features);
  const setFeature = useAppStore((s) => s.setFeature);
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
        desc="Tools for working on Fletch itself. Only available in development builds."
      />

      <SetGroup label="Home">
        <SetRow
          title="Mission Control"
          sub="Use the fleet review queue as the Home view. When off, Home is the quick-actions landing screen."
        >
          <SetToggle
            on={!!features.missionControl}
            onClick={() => setFeature("missionControl", !features.missionControl)}
          />
        </SetRow>
      </SetGroup>

      <SetGroup label="Onboarding">
        <SetRow
          title="Welcome tour"
          sub="Replay the cinematic onboarding — the feature tour and first-project walkthrough."
        >
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

      <SetGroup label="Updates" last>
        <SetRow
          title="Update-ready toast"
          sub="Show the “Update ready” restart toast with a fake version, without releasing a build. Restart now will actually relaunch."
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
