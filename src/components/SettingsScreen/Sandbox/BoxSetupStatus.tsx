import { useEffect } from "react";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { boxSetup } from "./boxSetup";

/** One compact line under the Developer pane's box toggle saying where the
 *  host's setup stands — "Downloading image 43% · Cancel", "Installing Claude
 *  Code tools…", "Ready · image v0.1.0 · Claude Code 2.1.312", or the error
 *  with Retry. Reads the probe once on mount (the image and the tools disks
 *  arrive as `box:image` / `box:tools` after that). Mounted only while the
 *  flag is on; the only surface for the install state. */
export function BoxSetupStatus() {
  const probe = useAppStore((s) => s.boxProbe);
  const image = useAppStore((s) => s.boxImage);
  const tools = useAppStore((s) => s.boxTools);
  const refreshProbe = useAppStore((s) => s.refreshBoxProbe);
  const refreshImage = useAppStore((s) => s.refreshBoxImage);
  const install = useAppStore((s) => s.installBoxImage);
  const cancel = useAppStore((s) => s.cancelBoxImage);

  useEffect(() => {
    void refreshProbe();
    void refreshImage();
  }, [refreshProbe, refreshImage]);

  const setup = boxSetup(probe, image, tools);
  return (
    <div className="set-row-s text-sm set-box-setup" data-state={setup.state}>
      <span>{setup.text}</span>
      {setup.state === "busy" && setup.cancel && (
        <Button variant="ghost" onClick={() => void cancel()}>
          Cancel
        </Button>
      )}
      {setup.state === "error" && (
        <Button variant="ghost" onClick={() => void install()}>
          Retry
        </Button>
      )}
    </div>
  );
}
