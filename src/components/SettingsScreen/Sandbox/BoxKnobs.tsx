import { useEffect, useState } from "react";
import type { BoxSettingKey } from "@/api";
import { TextInput } from "@/components/ui/TextInput";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { SetGroup, SetRow, SetToggle } from "../primitives";

/** The four text box knobs, in display order. Each maps to a host-owned
 *  `box_*` setting written through `set_box_setting`, which validates it; the
 *  defaults quoted here are the engine's (`sandbox/boxes/settings.rs`). */
const FIELDS = [
  {
    key: "box_memory_mib",
    title: "Memory (MiB)",
    sub: "Guest RAM per box. Blank uses the default (2048); at least 512.",
    placeholder: "2048",
  },
  {
    key: "box_cpus",
    title: "CPUs",
    sub: "vCPUs per box. Blank uses the default (2).",
    placeholder: "2",
  },
  {
    key: "box_idle_stop_minutes",
    title: "Idle stop (minutes)",
    sub: "Stop a box with no attached process after this long. Blank uses the default (10); 0 never stops.",
    placeholder: "10",
  },
  {
    key: "box_image_url",
    title: "Image URL",
    sub: "Alternate base URL for the image download (https://, http:// or file://). Blank uses the GitHub release.",
    placeholder: "https://github.com/… (built-in)",
  },
] as const satisfies readonly {
  key: BoxSettingKey;
  title: string;
  sub: string;
  placeholder: string;
}[];

type FieldKey = (typeof FIELDS)[number]["key"];

/** The optional image layers (`box_layers`), in the engine's order. A layer
 *  turned on is downloaded once and attached to every box from its next boot. */
const LAYERS = [
  {
    name: "devtools",
    title: "Developer tools layer",
    sub: "gcc, make, python3, pip and the Node headers: projects that build native code. On by default (~115 MB).",
  },
  {
    name: "browser",
    title: "Browser layer",
    sub: "A system Chromium (headless shell) and the Noto fonts for agents that drive a browser directly. Playwright projects download their own browser and work without it (~165 MB).",
  },
] as const;
const DEFAULT_LAYERS: readonly string[] = ["devtools"];

/** The layers a `box_layers` value turns on: blank is the default set, `none` nothing. */
export function enabledLayers(value: string): string[] {
  const v = value.trim();
  if (v === "") return [...DEFAULT_LAYERS];
  if (v === "none") return [];
  return v
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

/** The `box_layers` value for a set of layers, in the engine's order. */
export function layersValue(names: readonly string[]): string {
  const ordered = LAYERS.map((l) => l.name).filter((n) => names.includes(n));
  return ordered.length === 0 ? "none" : ordered.join(",");
}

/** One box knob: a draft committed on blur/Enter, with the host's validation
 *  message under it until the next edit. The stored value comes back through
 *  `settings:changed`, so the field never shows a value that did not stick. */
function BoxKnob({ field, value }: { field: (typeof FIELDS)[number]; value: string }) {
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

/** One layer toggle. Flipping it writes the whole `box_layers` value; the
 *  download of a layer turned on starts at once (the host does that). */
function LayerToggle({ layer, value }: { layer: (typeof LAYERS)[number]; value: string }) {
  const save = useAppStore((s) => s.saveBoxSetting);
  const hostGate = useGate("hostSettings");
  const [error, setError] = useState<string | null>(null);
  const enabled = enabledLayers(value);
  const on = enabled.includes(layer.name);
  const flip = async () => {
    const next = on ? enabled.filter((n) => n !== layer.name) : [...enabled, layer.name];
    setError(await save("box_layers", layersValue(next)));
  };
  return (
    <SetRow title={layer.title} sub={hostGate ?? error ?? layer.sub}>
      <SetToggle on={on} disabled={!!hostGate} tip={hostGate ?? undefined} onClick={() => void flip()} />
    </SetRow>
  );
}

/** The box sandbox's launch knobs. Mounted only while the box flag is on. */
export function BoxKnobs({ last }: { last?: boolean }) {
  // One selector per value, as ContainerLaunchKnobs does: a selector returning
  // a fresh object would hand `useSyncExternalStore` a new snapshot each render.
  const values: Record<FieldKey, string> = {
    box_memory_mib: useAppStore((s) => s.boxMemoryMib),
    box_cpus: useAppStore((s) => s.boxCpus),
    box_idle_stop_minutes: useAppStore((s) => s.boxIdleStopMinutes),
    box_image_url: useAppStore((s) => s.boxImageUrl),
  };
  const layers = useAppStore((s) => s.boxLayers);
  return (
    <SetGroup label="Box" last={last}>
      {LAYERS.map((l) => (
        <LayerToggle key={l.name} layer={l} value={layers} />
      ))}
      {FIELDS.map((f) => (
        <BoxKnob key={f.key} field={f} value={values[f.key]} />
      ))}
    </SetGroup>
  );
}
