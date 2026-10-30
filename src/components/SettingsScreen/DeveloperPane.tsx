import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { SetGroup, SetHead, SetRow, SetToggle } from "./primitives";
import { BoxSetupStatus } from "./Sandbox/BoxSetupStatus";

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
  const boxesEnabled = useAppStore((s) => s.boxesEnabled);
  const setBoxesEnabled = useAppStore((s) => s.setBoxesEnabled);
  const boxProbe = useAppStore((s) => s.boxProbe);
  const refreshBoxProbe = useAppStore((s) => s.refreshBoxProbe);
  const [refreshingModels, setRefreshingModels] = useState(false);

  // Probed with the flag off too: an Intel Mac, or one without the runtime,
  // gets a disabled toggle that says why rather than a refusal on click.
  useEffect(() => {
    void refreshBoxProbe();
  }, [refreshBoxProbe]);
  const boxRuntimeProblem =
    boxProbe && !boxProbe.runtime_ok ? boxProbe.problems.join("; ") : undefined;

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

      <SetGroup label="Pilots">
        <SetRow
          title="Project context layer"
          sub="Agents read and record project knowledge (vision, features, decisions, constraints) through context_* ops; a background pass extracts it from sessions and merged PRs. Adds a Context tab to every project page."
        >
          <SetToggle
            on={contextLayerEnabled}
            onClick={() => setContextLayerEnabled(!contextLayerEnabled)}
          />
        </SetRow>
        <SetRow
          title="Box sandbox (micro-VM)"
          sub="New Claude Code and Codex agents run in isolated micro-VMs; other providers keep the sandbox you had selected, for now. Turning this on downloads the 850 MB image once and switches new agents to boxes."
        >
          <SetToggle
            on={boxesEnabled}
            onClick={() => void setBoxesEnabled(!boxesEnabled)}
            disabled={!!boxRuntimeProblem && !boxesEnabled}
            tip={boxRuntimeProblem}
          />
        </SetRow>
        {boxesEnabled && <BoxSetupStatus />}
      </SetGroup>

      <SetGroup label="Onboarding">
        <SetRow
          title="Onboarding"
          sub="Replay the first-run onboarding — the welcome and the pre-flight check."
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

      <SetGroup label="Notifications" last>
        <SetRow
          title="Feedback prompt"
          sub="Show the one-time “How's Fletch working for you?” prompt that normally comes up on the third day of use. Triggering it here leaves its schedule untouched."
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
