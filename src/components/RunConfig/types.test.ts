import { describe, expect, it } from "vitest";
import { type SetupRow, withFallbackRows } from "./types";

const detected = (id: string, group: string, value: string): SetupRow => ({
  id,
  group,
  key: id,
  value,
  source: `detected ${id}`,
});

describe("withFallbackRows", () => {
  it("renders every core row empty when nothing was detected", () => {
    const rows = withFallbackRows([]);
    expect(rows.map((r) => r.id)).toEqual([
      "version",
      "install",
      "dev",
      "test",
      "lint",
      "build",
      "port",
    ]);
    expect(rows.every((r) => r.value === "" && r.placeholder)).toBe(true);
  });

  it("keeps undetected core rows editable alongside detected ones", () => {
    // A Node repo with dev/build scripts but no test script.
    const rows = withFallbackRows([
      detected("dev", "Scripts", "pnpm dev"),
      detected("build", "Scripts", "pnpm run build"),
    ]);
    const test = rows.find((r) => r.id === "test");
    expect(test).toMatchObject({ value: "", group: "Scripts", source: "" });
    expect(rows.find((r) => r.id === "dev")).toMatchObject({
      value: "pnpm dev",
      source: "detected dev",
    });
    expect(rows).toHaveLength(7);
  });

  it("appends detected rows outside the core set", () => {
    const rows = withFallbackRows([detected("env", "Server", ".env.local")]);
    expect(rows.at(-1)).toMatchObject({ id: "env", value: ".env.local" });
    expect(rows).toHaveLength(8);
  });
});
