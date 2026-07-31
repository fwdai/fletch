import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import type { FeedbackState } from "./useFeedback";

/** Format an encoded size for the thumbnail caption. */
function kb(bytes: number): string {
  return `${Math.round(bytes / 1024)} KB`;
}

/** Optional screenshot attachment: a browse button until something is picked,
 *  then a thumbnail with a remove affordance. The image is downscaled and
 *  JPEG-encoded on pick (see `util/image.ts`), so what's previewed here is
 *  exactly what gets sent. */
export function ScreenshotField({ state }: { state: FeedbackState }) {
  const { shot, shotError, attachScreenshot, removeScreenshot, status } = state;
  // The `sent` state replaces the whole form, so `sending` is the only in-form
  // state that has to lock the attachment controls.
  const busy = status === "sending";

  return (
    <div className="fb-field">
      <span className="fb-label text-sm">
        Screenshot <span className="fb-opt">optional</span>
      </span>

      {shot ? (
        <div className="fb-shot flex-center">
          <img className="fb-shot-thumb" src={shot.dataUrl} alt={`Attached: ${shot.name}`} />
          <div className="fb-shot-meta">
            <div className="fb-shot-name text-sm">{shot.name}</div>
            <div className="fb-shot-size mono text-xs">{kb(shot.bytes)}</div>
          </div>
          <IconButton size="sm" tip="Remove screenshot" disabled={busy} onClick={removeScreenshot}>
            <Icon name="close" size={13} />
          </IconButton>
        </div>
      ) : (
        <div className="fb-shot-empty flex-center">
          <Button
            variant="outline"
            size="sm"
            disabled={busy}
            onClick={() => void attachScreenshot()}
          >
            <Icon name="attach" size={13} />
            Attach a screenshot
          </Button>
          <span className="fb-hint text-sm">Take one with ⌘⇧4, then pick the file.</span>
        </div>
      )}

      {shotError && <div className="fb-hint e text-sm">{shotError}</div>}
    </div>
  );
}
