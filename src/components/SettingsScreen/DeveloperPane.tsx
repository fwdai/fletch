import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { SetGroup, SetHead, SetRow } from "./primitives";

/** Dev-only settings surface. The nav entry that routes here is gated on
 *  `import.meta.env.DEV`, so this pane only ships under `bun tauri dev`. */
export function DeveloperPane() {
  const openOnboarding = useAppStore((s) => s.openOnboarding);
  const closeSettingsScreen = useAppStore((s) => s.closeSettingsScreen);
  const setUpdateReady = useAppStore((s) => s.setUpdateReady);

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Developer"
        title="Developer"
        desc="Tools for working on Fletch itself. Only available in development builds."
      />

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
