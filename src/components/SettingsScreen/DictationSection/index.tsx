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
      {/* Always offered, engine on or off: the choice has to be makeable
          before the opt-in, or enabling would fetch the platform default out
          from under someone who wanted the other model — and weights on disk
          outlive the opt-in, so the Remove action has to stay reachable too. */}
      {status?.models.map((model) => (
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
