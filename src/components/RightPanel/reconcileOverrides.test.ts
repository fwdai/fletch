import { describe, expect, it } from "vitest";
import type { SetupRow } from "./RunSettingsSheet";
import { reconcileOverrides } from "./reconcileOverrides";

const row = (id: string, value: string): SetupRow => ({
  id,
  group: "Scripts",
  key: id,
  value,
  source: "test",
});

const ROWS: SetupRow[] = [row("dev", "pnpm dev"), row("port", "5173")];

describe("reconcileOverrides", () => {
  it("keeps and persists a value that differs from the detected default", () => {
    const r = reconcileOverrides(ROWS, {}, { dev: "pnpm start" });
    expect(r.cleaned).toEqual({ dev: "pnpm start" });
    expect(r.toSet).toEqual([{ id: "dev", value: "pnpm start" }]);
    expect(r.toDelete).toEqual([]);
  });

  it("drops and deletes a value equal to the detected default", () => {
    const r = reconcileOverrides(ROWS, { dev: "pnpm start" }, { dev: "pnpm dev" });
    expect(r.cleaned).toEqual({});
    expect(r.toSet).toEqual([]);
    expect(r.toDelete).toEqual(["dev"]);
  });

  it("does not re-write an unchanged override", () => {
    const r = reconcileOverrides(ROWS, { dev: "pnpm start" }, { dev: "pnpm start" });
    expect(r.cleaned).toEqual({ dev: "pnpm start" });
    expect(r.toSet).toEqual([]);
    expect(r.toDelete).toEqual([]);
  });

  it("prunes a stale override whose row no longer exists in detection", () => {
    // The project flipped ecosystems: `port` was overridden under the old
    // config but the new detected rows have no `port` row. The stale key
    // must be deleted, not left dangling in the DB.
    const newRows: SetupRow[] = [row("dev", "cargo run")];
    const r = reconcileOverrides(newRows, { port: "4000" }, { port: "4000" });
    expect(r.cleaned).toEqual({});
    expect(r.toDelete).toEqual(["port"]);
    expect(r.toSet).toEqual([]);
  });

  it("clears a stale override via reset-all (empty next)", () => {
    const newRows: SetupRow[] = [row("dev", "cargo run")];
    const r = reconcileOverrides(newRows, { port: "4000" }, {});
    expect(r.cleaned).toEqual({});
    expect(r.toDelete).toEqual(["port"]);
  });

  it("leaves rows without overrides untouched", () => {
    const r = reconcileOverrides(ROWS, {}, {});
    expect(r.cleaned).toEqual({});
    expect(r.toSet).toEqual([]);
    expect(r.toDelete).toEqual([]);
  });

  it("handles set and prune together in one apply", () => {
    const newRows: SetupRow[] = [row("dev", "cargo run")];
    const r = reconcileOverrides(
      newRows,
      { port: "4000" }, // stale, to delete
      { port: "4000", dev: "cargo run --release" }, // new dev override
    );
    expect(r.cleaned).toEqual({ dev: "cargo run --release" });
    expect(r.toSet).toEqual([{ id: "dev", value: "cargo run --release" }]);
    expect(r.toDelete).toEqual(["port"]);
  });
});
