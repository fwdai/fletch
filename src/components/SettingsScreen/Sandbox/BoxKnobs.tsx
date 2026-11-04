import { useEffect, useState } from "react";
import type { BoxSettingKey } from "@/api";
import { Scale } from "@/components/ui/Scale";
import { TextInput } from "@/components/ui/TextInput";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { SetGroup, SetRow } from "../primitives";
import { BOX_SCALES, type BoxScale, scaleStops, scaleValue } from "./boxScales";

/** One text box knob: a host-owned `box_*` setting written through
 *  `set_box_setting`, which validates it. */
interface KnobField {
  key: BoxSettingKey;
  title: string;
  sub: string;
  placeholder: string;
}

/** Where the image downloads from: on Settings › Developer, since it exists to
 *  boot a locally built image (`file://`). */
export const IMAGE_URL_FIELD: KnobField = {
  key: "box_image_url",
  title: "Linux VM image URL",
  sub: "Base URL for the VM image (https, http or file). Blank uses the GitHub release.",
  placeholder: "https://github.com/… (built-in)",
};

/** One box knob: a draft committed on blur/Enter, with the host's validation
 *  message under it until the next edit. The stored value comes back through
 *  `settings:changed`, so the field never shows a value that did not stick. */
export function BoxKnob({ field, value }: { field: KnobField; value: string }) {
  const save = useAppStore((s) => s.saveBoxSetting);
  const hostGate = useGate("hostSettings");
  const [draft, setDraft] = useState(value);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(value);
  }, [value]);

  const commit = async () => {
    const next = draft.trim();
    if (next === value) return;
    setError(await save(field.key, next));
  };

  return (
    <SetRow title={field.title} sub={hostGate ?? error ?? field.sub}>
      <TextInput
        mono
        value={draft}
        disabled={!!hostGate}
        placeholder={field.placeholder}
        spellCheck={false}
        invalid={!!error}
        onChange={(e) => {
          setError(null);
          setDraft(e.target.value);
        }}
        onBlur={() => void commit()}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
        }}
      />
    </SetRow>
  );
}

/** One numeric box knob as a scale. A pick is saved at once; the stored value
 *  comes back through `settings:changed`, so the scale never shows a value
 *  that did not stick. */
function BoxScaleRow({ scale, stored }: { scale: BoxScale; stored: string }) {
  const save = useAppStore((s) => s.saveBoxSetting);
  const hostCpus = useAppStore((s) => s.boxProbe?.host_cpus);
  const hostMemoryMib = useAppStore((s) => s.boxProbe?.host_memory_mib);
  const hostGate = useGate("hostSettings");
  const [error, setError] = useState<string | null>(null);
  const value = scaleValue(scale, stored);

  const pick = async (next: string) => {
    if (next === String(value)) return;
    setError(await save(scale.key, next));
  };

  return (
    <SetRow title={scale.title} sub={hostGate ?? error ?? scale.sub}>
      <Scale
        className="set-scale"
        ariaLabel={scale.title}
        stops={scaleStops(scale, value, { host_cpus: hostCpus, host_memory_mib: hostMemoryMib })}
        value={String(value)}
        disabled={!!hostGate}
        onPick={(next) => void pick(next)}
      />
    </SetRow>
  );
}

/** The Linux VM's resources. Mounted only where the machine can boot one. */
export function BoxKnobs({ last }: { last?: boolean }) {
  // One selector per value: a selector returning a fresh object would hand
  // `useSyncExternalStore` a new snapshot each render.
  const stored: Partial<Record<BoxScale["key"], string>> = {
    box_memory_mib: useAppStore((s) => s.boxMemoryMib),
    box_cpus: useAppStore((s) => s.boxCpus),
    box_disk_gib: useAppStore((s) => s.boxDiskGib),
    box_idle_stop_minutes: useAppStore((s) => s.boxIdleStopMinutes),
  };
  return (
    <SetGroup label="Resources" last={last}>
      {BOX_SCALES.map((scale) => (
        <BoxScaleRow key={scale.key} scale={scale} stored={stored[scale.key] ?? ""} />
      ))}
    </SetGroup>
  );
}
