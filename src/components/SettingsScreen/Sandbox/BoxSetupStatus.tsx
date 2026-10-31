import { useEffect } from "react";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { boxSetup } from "./boxSetup";

/** One compact line under the isolated-VM switch saying where the host's
 *  setup stands — "Downloading image 43% · Cancel", "Installing Claude Code
 *  tools…", "Ready · image v0.2.0 · Claude Code 2.1.312", or the error with
 *  Retry (Download, for an image that was never fetched). Reads the image
 *  once on mount (the probe is the pane's; `box:image` / `box:tools` carry
 *  every change after). Mounted only while the switch is on and the machine
 *  can boot a box; the one surface for the install state. */
export function BoxSetupStatus() {
  const probe = useAppStore((s) => s.boxProbe);
  const image = useAppStore((s) => s.boxImage);
  const tools = useAppStore((s) => s.boxTools);
  const refreshImage = useAppStore((s) => s.refreshBoxImage);
  const install = useAppStore((s) => s.installBoxImage);
  const cancel = useAppStore((s) => s.cancelBoxImage);
  const retryTools = useAppStore((s) => s.retryBoxTools);

  useEffect(() => {
    void refreshImage();
  }, [refreshImage]);

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
        <Button
          variant="ghost"
          onClick={() => void (setup.retry === "tools" ? retryTools(setup.provider) : install())}
        >
          {image?.phase === "not_installed" ? "Download" : "Retry"}
        </Button>
      )}
    </div>
  );
}
