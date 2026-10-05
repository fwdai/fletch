import { miscApi } from "@/api/domains/misc";
import { FletchMark } from "@/components/FletchMark";
import { Button } from "@/components/ui/Button";
import { Loader } from "@/components/ui/Loader";
import { BOOT_PHASE_LABELS, type BootStatus } from "@/util/boot";

/** What the window shows until the engine is up (or has given up). Styled in
 *  fixed colors: the theme class is a setting, and settings live behind the
 *  engine this screen is waiting for. */
export function BootScreen({ status }: { status: BootStatus }) {
  return (
    <div className="boot-screen" data-tauri-drag-region>
      <FletchMark className="boot-mark" />
      {status.phase === "failed" ? (
        <>
          <div className="boot-title">Fletch couldn't start</div>
          <div className="boot-message">{status.message}</div>
          <Button variant="outline" onClick={() => void miscApi.revealLogs().catch(() => {})}>
            Reveal logs
          </Button>
        </>
      ) : (
        <div className="boot-step">
          <Loader variant="inherit" aria-hidden />
          {status.phase === "booting" ? BOOT_PHASE_LABELS[status.step] : "Starting"}
        </div>
      )}
    </div>
  );
}
