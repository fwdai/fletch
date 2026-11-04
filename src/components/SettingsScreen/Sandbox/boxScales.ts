import type { BoxSettingKey } from "@/api";
import type { ScaleStop } from "@/components/ui/Scale";

/** What the host has, from `box_runtime_probe`. Absent on a host from before
 *  those fields, or where its OS will not say. */
export interface HostResources {
  host_cpus?: number | null;
  host_memory_mib?: number | null;
}

/** One numeric box setting, picked from a scale. `engineDefault` is what the
 *  engine uses when the setting is unset (`sandbox/boxes/settings.rs`). */
export interface BoxScale {
  key: BoxSettingKey;
  title: string;
  sub: string;
  engineDefault: number;
  stops: (host: HostResources) => number[];
  label: (value: number) => string;
}

const MEMORY_STOPS_MIB = [1024, 2048, 4096, 8192, 16384, 32768, 65536];
const CPU_STOPS = [1, 2, 4, 6, 8, 12, 16];
const IDLE_STOPS_MINUTES = [5, 10, 30, 60, 0];

/** Without word from the host, what any machine that boots a box can give. */
const UNKNOWN_HOST_MEMORY_MIB = 16384;
const UNKNOWN_HOST_CPUS = 8;

function formatMemory(mib: number): string {
  return `${Number((mib / 1024).toFixed(1))} GB`;
}

function formatIdle(minutes: number): string {
  if (minutes === 0) return "Never";
  return minutes % 60 === 0 ? `${minutes / 60} hr` : `${minutes} min`;
}

/** The scales on Settings › Sandbox, in display order. Memory stops at half the
 *  host: it is a cap per VM and several run at once. */
export const BOX_SCALES: BoxScale[] = [
  {
    key: "box_memory_mib",
    title: "Memory",
    sub: "RAM each VM can use, from its next start.",
    engineDefault: 2048,
    stops: ({ host_memory_mib }) => {
      const limit = host_memory_mib ? host_memory_mib / 2 : UNKNOWN_HOST_MEMORY_MIB;
      return MEMORY_STOPS_MIB.filter((mib) => mib <= limit);
    },
    label: formatMemory,
  },
  {
    key: "box_cpus",
    title: "CPUs",
    sub: "Cores each VM can use, from its next start.",
    engineDefault: 2,
    stops: ({ host_cpus }) => CPU_STOPS.filter((n) => n <= (host_cpus || UNKNOWN_HOST_CPUS)),
    label: String,
  },
  {
    key: "box_idle_stop_minutes",
    title: "Idle stop",
    sub: "Stop a VM after this long with nothing running.",
    engineDefault: 10,
    stops: () => IDLE_STOPS_MINUTES,
    label: formatIdle,
  },
];

/** The value in force: the stored one, or the engine's default when it is
 *  unset or unreadable (the engine reads a bad row as its default too). */
export function scaleValue(scale: BoxScale, stored: string): number {
  const value = Number.parseInt(stored, 10);
  return Number.isNaN(value) ? scale.engineDefault : value;
}

/** Never sits past every number of minutes. */
const rank = (value: number) => (value === 0 ? Number.POSITIVE_INFINITY : value);

/** The stops to offer. The value in force is always one of them, even off the
 *  list or past what the host has, so the scale shows what is really set
 *  instead of quietly reading as a neighbour. */
export function scaleStops(scale: BoxScale, value: number, host: HostResources): ScaleStop[] {
  return [...new Set([...scale.stops(host), value])]
    .sort((a, b) => rank(a) - rank(b))
    .map((n) => ({ value: String(n), label: scale.label(n) }));
}
