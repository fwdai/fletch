import { describe, expect, it } from "vitest";
import { BOX_SCALES, type BoxScale, scaleStops, scaleValue } from "./boxScales";

function scale(key: BoxScale["key"]): BoxScale {
  const found = BOX_SCALES.find((s) => s.key === key);
  if (!found) throw new Error(`no scale for ${key}`);
  return found;
}

const labels = (s: BoxScale, value: number, host = {}) =>
  scaleStops(s, value, host).map((stop) => stop.label);

describe("scaleValue", () => {
  it("reads the stored value, 0 included", () => {
    expect(scaleValue(scale("box_memory_mib"), "4096")).toBe(4096);
    expect(scaleValue(scale("box_idle_stop_minutes"), "0")).toBe(0);
  });

  it("falls back to the engine's default when unset or unreadable", () => {
    expect(scaleValue(scale("box_memory_mib"), "")).toBe(2048);
    expect(scaleValue(scale("box_cpus"), "lots")).toBe(2);
  });

  it("reads the whole string, as the engine does", () => {
    expect(scaleValue(scale("box_memory_mib"), " 4096 ")).toBe(4096);
    expect(scaleValue(scale("box_memory_mib"), "+4096")).toBe(4096);
    for (const bad of ["4096oops", "1.5", "-4096", "0x1000", "1e4"]) {
      expect(scaleValue(scale("box_memory_mib"), bad)).toBe(2048);
    }
  });

  it("applies the engine's bounds for each setting", () => {
    const edges: [BoxScale["key"], string, number][] = [
      ["box_memory_mib", "512", 512],
      ["box_memory_mib", "511", 2048],
      ["box_memory_mib", "4294967296", 2048],
      ["box_cpus", "1", 1],
      ["box_cpus", "32", 32],
      ["box_cpus", "0", 2],
      ["box_cpus", "33", 2],
      ["box_disk_gib", "4", 4],
      ["box_disk_gib", "256", 256],
      ["box_disk_gib", "3", 8],
      ["box_disk_gib", "257", 8],
      ["box_idle_stop_minutes", "1440", 1440],
      ["box_idle_stop_minutes", "1441", 10],
    ];
    for (const [key, stored, expected] of edges) {
      expect(scaleValue(scale(key), stored)).toBe(expected);
    }
  });
});

describe("scaleStops", () => {
  it("stops memory at half the host, since several VMs run at once", () => {
    expect(labels(scale("box_memory_mib"), 2048, { host_memory_mib: 16384 })).toEqual([
      "1 GB",
      "2 GB",
      "4 GB",
      "8 GB",
    ]);
  });

  it("stops CPUs at the host's cores", () => {
    expect(labels(scale("box_cpus"), 2, { host_cpus: 4 })).toEqual(["1", "2", "4"]);
  });

  it("offers a modest range for a host that does not say", () => {
    expect(labels(scale("box_memory_mib"), 2048).at(-1)).toBe("16 GB");
    expect(labels(scale("box_cpus"), 2).at(-1)).toBe("8");
  });

  it("keeps a value that is off the list, in order", () => {
    expect(labels(scale("box_memory_mib"), 3072, { host_memory_mib: 16384 })).toEqual([
      "1 GB",
      "2 GB",
      "3 GB",
      "4 GB",
      "8 GB",
    ]);
    expect(labels(scale("box_memory_mib"), 512, { host_memory_mib: 16384 })[0]).toBe("0.5 GB");
  });

  it("keeps a value past what the host has", () => {
    expect(labels(scale("box_cpus"), 32, { host_cpus: 4 }).at(-1)).toBe("32");
  });

  it("puts Never last, past any number of minutes", () => {
    expect(labels(scale("box_idle_stop_minutes"), 10)).toEqual([
      "5 min",
      "10 min",
      "30 min",
      "1 hr",
      "Never",
    ]);
    expect(labels(scale("box_idle_stop_minutes"), 120).slice(-2)).toEqual(["2 hr", "Never"]);
  });

  it("does not repeat a value already on the list", () => {
    expect(labels(scale("box_idle_stop_minutes"), 0)).toHaveLength(5);
  });
});
