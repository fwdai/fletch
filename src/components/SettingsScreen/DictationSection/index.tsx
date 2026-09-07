import { useAppStore } from "@/store";
import { formatBytes } from "@/util/format";
import { SetGroup, SetRow, SetToggle } from "../primitives";
import { ModelRow } from "./ModelRow";
import { useDictationModel } from "./useDictationModel";

/** Settings › General › Dictation: opt into the local Whisper engine instead
 *  of Apple's recognizer, and pick which model it runs. The opt-in and the
 *  weights are separate states — the toggle flips immediately and the model
 *  downloads behind it — so the chooser below is the only place that says
 *  whether dictation can actually run locally yet. macOS-only, like the
 *  recognizer it replaces. */
export function DictationSection() {
  const enabled = useAppStore((s) => s.dictationEngineEnabled);
  const setEnabled = useAppStore((s) => s.setDictationEngineEnabled);
  const { status, progress, select, download, remove } = useDictationModel();

  const size = status ? formatBytes(status.model.size) : null;
  // Hidden while there is nothing to act on, and shown as soon as there is:
  // weights on disk outlive the opt-in, so turning the engine off must not
  // strand half a gigabyte with no way to reclaim it.
  const chooser =
    status &&
    (enabled ||
      status.downloading ||
      progress?.state === "error" ||
      status.models.some((m) => m.installed));

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
      {chooser &&
        status.models.map((model) => (
          <ModelRow
            key={model.id}
            model={model}
            status={status}
            progress={progress}
            onSelect={() => select(model.id)}
            onDownload={() => download(model.id)}
            onRemove={async () => {
              // Deleting the weights under an enabled engine would leave it
              // silently falling back, so the opt-in goes first — and only if
              // it actually persisted do the weights follow. A model that
              // isn't the selected one is nothing the engine would load.
              if (enabled && model.id === status.model.id && !(await setEnabled(false))) return;
              remove(model.id);
            }}
          />
        ))}
    </SetGroup>
  );
}
