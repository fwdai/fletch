import { useAppStore } from "@/store";
import { formatBytes } from "@/util/format";
import { SetGroup, SetHead, SetRow, SetToggle } from "../primitives";
import { ModelRow } from "./ModelRow";
import { useDictationModel } from "./useDictationModel";

/** Settings › Dictation: opt into the local Whisper engine instead of Apple's
 *  recognizer, and pick which model it runs. The opt-in and the weights are
 *  separate states — the toggle flips immediately and the model downloads
 *  behind it — so the chooser below is the only place that says whether
 *  dictation can actually run locally yet. macOS-only, like the recognizer it
 *  replaces; the nav entry is hidden elsewhere. */
export function DictationPane() {
  const enabled = useAppStore((s) => s.dictationEngineEnabled);
  const setEnabled = useAppStore((s) => s.setDictationEngineEnabled);
  const autoStop = useAppStore((s) => s.dictationAutoStop);
  const setAutoStop = useAppStore((s) => s.setDictationAutoStop);
  const { status, progress, select, download, remove } = useDictationModel();

  const size = status ? formatBytes(status.model.size) : null;

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Dictation"
        title="Dictation"
        desc="Speak instead of typing in the composer. Choose the engine and the model it runs."
      />

      <SetGroup label="Listening">
        <SetRow
          title="Stop after a pause"
          sub="Finish dictating on its own after a two-second pause. Off means you stop it manually."
        >
          <SetToggle on={autoStop} onClick={() => setAutoStop(!autoStop)} />
        </SetRow>
      </SetGroup>

      <SetGroup label="Speech recognition" last>
        <SetRow
          title="Local speech recognition"
          sub={`Transcribe with Whisper on this Mac instead of Apple's recognizer.${
            size ? ` One-time ${size} download.` : ""
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
    </div>
  );
}
