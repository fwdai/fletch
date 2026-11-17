import { useEffect, useRef, useState } from "react";
import { useAppStore } from "@/store";
import { useHostWhere } from "@/store/capabilities";
import { EachHostScope } from "@/store/EngineScope";
import { Icon } from "./Icon";
import { boxSetup } from "./SettingsScreen/Sandbox/boxSetup";
import { Button } from "./ui/Button";

/** How long the toast stays before leaving on its own: a notice, not a
 *  decision. */
const LINGER_MS = 12_000;

/**
 * Bottom-left toast, once: the Linux VM this session watched being set up
 * (the image download, the tools disk) is ready, so new workspaces run in it
 * from now on. Only after a setup seen in flight — a launch that finds the VM
 * already ready has nothing to announce — and never for a failure, which
 * Settings › Sandbox carries with its Retry. Fed by `box:image` / `box:tools`
 * through the store, for every host at once (This Mac's first), each watched
 * in its own scope: a download watched on one host is not news about another.
 */
export function BoxReadyToast() {
  return (
    <div className="docker-build-stack">
      <EachHostScope>
        <HostBoxReady />
      </EachHostScope>
    </div>
  );
}

/** The scope's host's toast. */
function HostBoxReady() {
  const probe = useAppStore((s) => s.boxProbe);
  const image = useAppStore((s) => s.boxImage);
  const tools = useAppStore((s) => s.boxTools);
  const where = useHostWhere();
  const [open, setOpen] = useState(false);
  // The image was seen on its way this session; the next "ready" is news. A
  // tools disk rebuilt after a CLI update (busy, image already in) is not:
  // boxes were the engine before it and are after it.
  const sawDownload = useRef(false);

  const state = probe?.runtime_ok && image ? boxSetup(probe, image, tools).state : null;
  const downloading = state === "busy" && image?.phase !== "installed";
  useEffect(() => {
    if (downloading) sawDownload.current = true;
    if (state === "ready" && sawDownload.current) {
      sawDownload.current = false;
      setOpen(true);
    }
  }, [state, downloading]);

  useEffect(() => {
    if (!open) return;
    const t = window.setTimeout(() => setOpen(false), LINGER_MS);
    return () => window.clearTimeout(t);
  }, [open]);

  if (!open) return null;
  return (
    <div className="update-toast docker-build-toast" role="status">
      <Icon name="cube" />
      <div className="update-toast-body">
        <div className="update-toast-text">
          <strong>Linux VM is ready{where}</strong>
          <span>New workspaces run in it. The ones you have stay where they are.</span>
        </div>
        <div className="update-toast-actions">
          <Button variant="primary" onClick={() => setOpen(false)}>
            OK
          </Button>
        </div>
      </div>
    </div>
  );
}
