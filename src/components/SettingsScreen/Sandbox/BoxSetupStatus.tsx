import { useEffect } from "react";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { SetGroup } from "../primitives";
import { boxSetup } from "./boxSetup";

/** One compact line on Settings › Sandbox while the host's Linux VM setup needs
 *  attention: "Downloading image 43% · Cancel", "Installing Claude Code
 *  tools…", or the error with Retry (Download, for an image that was never
 *  fetched). A ready VM says nothing here; its versions are in `VmDetails`.
 *  Reads the image once on mount (the probe is the pane's; `box:image` /
 *  `box:tools` carry every change after). Mounted only where the machine can
 *  boot a box (or before the probe answers). */
export function BoxSetupStatus() {
  const probe = useAppStore((s) => s.boxProbe);
  const image = useAppStore((s) => s.boxImage);
  const tools = useAppStore((s) => s.boxTools);
  const refreshImage = useAppStore((s) => s.refreshBoxImage);
  const install = useAppStore((s) => s.installBoxImage);
  const cancel = useAppStore((s) => s.cancelBoxImage);
  const retryTools = useAppStore((s) => s.retryBoxTools);
  const canAct = !useGate("vmSetup");

  useEffect(() => {
    void refreshImage();
  }, [refreshImage]);

  const setup = boxSetup(probe, image, tools);
  if (setup.state === "ready" || setup.state === "checking") return null;
  return (
    <SetGroup>
      <div className="set-row-s text-sm set-box-setup" data-state={setup.state}>
        <span>{setup.text}</span>
        {canAct && setup.state === "busy" && setup.cancel && (
          <Button variant="ghost" onClick={() => void cancel()}>
            Cancel
          </Button>
        )}
        {canAct && setup.state === "error" && (
          <Button
            variant="ghost"
            onClick={() => void (setup.retry === "tools" ? retryTools(setup.provider) : install())}
          >
            {image?.phase === "not_installed" ? "Download" : "Retry"}
          </Button>
        )}
      </div>
    </SetGroup>
  );
}
