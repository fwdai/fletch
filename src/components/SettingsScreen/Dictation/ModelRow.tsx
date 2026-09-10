import type { DictationModel, DictationModelProgressEvent, DictationModelStatus } from "@/api";
import { SetRow } from "../primitives";
import { ModelStatusRow } from "./ModelStatusRow";

/** One catalog entry in the chooser: its copy, a radio that makes it the
 *  engine's model, and the line saying what its weights are doing. The radio
 *  sits in the row's control slot so it lines up with the toggle above it. */
export function ModelRow({
  model,
  status,
  progress,
  onSelect,
  onDownload,
  onRemove,
}: {
  model: DictationModel;
  status: DictationModelStatus;
  progress: DictationModelProgressEvent | null;
  onSelect: () => void;
  onDownload: () => void;
  onRemove: () => void;
}) {
  const selected = status.model.id === model.id;

  return (
    <SetRow
      align="start"
      title={model.label}
      sub={
        <>
          {model.note}
          <ModelStatusRow
            model={model}
            status={status}
            progress={progress}
            onDownload={onDownload}
            onRemove={onRemove}
          />
        </>
      }
    >
      <button
        type="button"
        className="set-radio"
        role="radio"
        aria-checked={selected}
        aria-label={`Use ${model.label}`}
        data-on={selected ? "1" : "0"}
        onClick={onSelect}
      >
        <i />
      </button>
    </SetRow>
  );
}
