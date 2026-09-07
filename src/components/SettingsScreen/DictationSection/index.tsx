import { useAppStore } from "@/store";
import { formatBytes } from "@/util/format";
import { SetGroup, SetRow, SetToggle } from "../primitives";
import { ModelStatusRow } from "./ModelStatusRow";
import { useDictationModel } from "./useDictationModel";

/** Settings › General › Dictation: opt into the local Whisper engine instead
 *  of Apple's recognizer. The opt-in and the weights are separate states — the
 *  toggle flips immediately and the model downloads behind it — so the row
 *  below the toggle is the only place that says whether dictation can actually
 *  run locally yet. macOS-only, like the recognizer it replaces. */
export function DictationSection() {
  const enabled = useAppStore((s) => s.dictationEngineEnabled);
  const setEnabled = useAppStore((s) => s.setDictationEngineEnabled);
  const { status, progress, download, remove } = useDictationModel();

  const size = status ? formatBytes(status.model.size) : null;

  return (
    <SetGroup label="Dictation">
      <SetRow
        title="Local speech recognition"
        sub={`Transcribe with OpenAI's Whisper on this Mac instead of Apple's recognizer.${
          size ? ` Downloads a ${size} model once.` : ""
        }`}
      >
        <SetToggle on={enabled} onClick={() => setEnabled(!enabled)} />
      </SetRow>
      {status && (
        <ModelStatusRow
          enabled={enabled}
          status={status}
          progress={progress}
          onDownload={download}
          onRemove={() => {
            // Deleting the weights under an enabled engine would leave it
            // silently falling back, so the opt-in goes with them.
            if (enabled) setEnabled(false);
            remove();
          }}
        />
      )}
    </SetGroup>
  );
}
