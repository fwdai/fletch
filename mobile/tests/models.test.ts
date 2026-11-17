// Picker rows in src/lib/models.ts. The regression this guards: the host
// reports zero models for agents whose CLI has no list command (claude answers
// `providerHint: "anthropic"` and an empty list), so reading the raw discovery
// left the Model sheet with nothing but its own "Default model" row.

import { buildCatalog } from "@desktop/data/modelCatalog/build";
import type { ModelsDevIndex } from "@desktop/data/modelCatalog/modelsDev";
import type { AgentModels } from "@desktop/data/modelCatalog/types";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { linkOf } from "../src/host";
import { CATALOG_TTL_MS, loadModels, modelsFor, modelsOf } from "../src/lib/models";

const mocks = vi.hoisted(() => ({ rebuildCatalog: vi.fn() }));

vi.mock("@desktop/data/modelCatalog", () => ({ rebuildCatalog: mocks.rebuildCatalog }));

/** A host key no earlier case has built a catalog for: the catalogs are kept
 *  for the life of the module, as they are for the life of the app. */
let hosts = 0;
const freshHost = () => linkOf(`models-host-${++hosts}`);

const index: ModelsDevIndex = {
  byId: {
    "claude-opus-9": {
      id: "claude-opus-9",
      name: "Claude Opus 9",
      contextWindow: 1_000_000,
      reasoning: true,
      family: "claude-opus",
      releaseDate: "2026-08-01",
    },
    "claude-haiku-9": {
      id: "claude-haiku-9",
      name: "Claude Haiku 9",
      contextWindow: 200_000,
      reasoning: false,
      family: "claude-haiku",
      releaseDate: "2026-07-01",
    },
  },
  byProvider: { anthropic: ["claude-opus-9", "claude-haiku-9"] },
};

describe("modelsFor", () => {
  it("expands a provider hint, so an agent with no list command is selectable", () => {
    const discovered: AgentModels[] = [{ agent: "claude", providerHint: "anthropic", models: [] }];
    const list = modelsFor(buildCatalog(discovered, index).byAgent, "claude");

    expect(list.map((m) => m.id)).toEqual(["", "claude-opus-9", "claude-haiku-9"]);
  });

  it("offers the default row first, whatever the catalog says", () => {
    const list = modelsFor({ codex: [] }, "codex");
    expect(list[0].id).toBe("");
    expect(list[0].name).toBe("Default model");
  });

  it("falls back to the static list when the catalog has nothing for a provider", () => {
    // An empty entry is what a failed models.dev fetch leaves behind; it must
    // not win over the stand-in the way an `?? ` on a non-null [] used to.
    expect(modelsFor({ claude: [] }, "claude").length).toBeGreaterThan(1);
    expect(modelsFor({}, "claude").length).toBeGreaterThan(1);
  });

  it("shows only the default for a provider nothing knows about", () => {
    expect(modelsFor({}, "antigravity").map((m) => m.id)).toEqual([""]);
  });
});

describe("loadModels", () => {
  const catalog = { byId: {}, byAgent: { codex: [] } };
  const onA = { byId: {}, byAgent: { claude: [] } };
  const onB = { byId: {}, byAgent: { codex: [] } };

  beforeEach(() => {
    mocks.rebuildCatalog.mockReset();
    mocks.rebuildCatalog.mockResolvedValue(catalog);
  });

  afterEach(() => vi.useRealTimers());

  it("builds each host a catalog of its own, from that host's discovery", async () => {
    // Half a catalog is the host's installed CLIs, so one host must never be
    // served another's lists.
    const a = freshHost();
    const b = freshHost();
    mocks.rebuildCatalog.mockImplementation(async (discover) =>
      discover === a.api.discoverSupportedModels ? onA : onB,
    );

    expect(await loadModels(a)).toEqual(onA.byAgent);
    expect(await loadModels(b)).toEqual(onB.byAgent);
    expect(modelsOf(a.hostKey)).toEqual(onA.byAgent);
    expect(modelsOf(b.hostKey)).toEqual(onB.byAgent);
    expect(mocks.rebuildCatalog).toHaveBeenCalledTimes(2);
  });

  it("serves a host its own catalog again, however often the other host builds", async () => {
    const a = freshHost();
    const b = freshHost();
    await loadModels(a);
    await loadModels(b);
    await loadModels(a);
    await loadModels(b);
    // One rebuild each: going back and forth between hosts rebuilds nothing.
    expect(mocks.rebuildCatalog).toHaveBeenCalledTimes(2);
    expect(JSON.parse(localStorage.getItem("modelCatalog.byHost") ?? "{}")).toMatchObject({
      [a.hostKey ?? ""]: { byAgent: catalog.byAgent },
      [b.hostKey ?? ""]: { byAgent: catalog.byAgent },
    });
  });

  it("rebuilds a host's catalog once it has aged out, and only that host's", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const a = freshHost();
    await loadModels(a);
    vi.advanceTimersByTime(CATALOG_TTL_MS / 2);
    const b = freshHost();
    await loadModels(b);
    vi.advanceTimersByTime(CATALOG_TTL_MS / 2 + 1);

    await loadModels(a);
    await loadModels(b);
    expect(mocks.rebuildCatalog).toHaveBeenCalledTimes(3);
  });

  it("shares one rebuild between two pickers on the same host, not across hosts", async () => {
    const a = freshHost();
    const b = freshHost();
    let finishA: (value: unknown) => void = () => {};
    mocks.rebuildCatalog.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishA = resolve;
        }),
    );

    const first = loadModels(a);
    const second = loadModels(a);
    // Another host's rebuild is its own, and lands while host a's is out.
    expect(await loadModels(b)).toEqual(catalog.byAgent);
    expect(mocks.rebuildCatalog).toHaveBeenCalledTimes(2);

    finishA(onA);
    expect(await first).toEqual(onA.byAgent);
    expect(await second).toEqual(onA.byAgent);
    expect(modelsOf(b.hostKey)).toEqual(catalog.byAgent);
  });

  it("keeps nothing for a host whose rebuild failed, and tries again next time", async () => {
    const a = freshHost();
    mocks.rebuildCatalog.mockResolvedValue(null);
    expect(await loadModels(a)).toBeNull();
    expect(modelsOf(a.hostKey)).toEqual({});

    mocks.rebuildCatalog.mockResolvedValue(catalog);
    expect(await loadModels(a)).toEqual(catalog.byAgent);
    expect(mocks.rebuildCatalog).toHaveBeenCalledTimes(2);
  });
});
