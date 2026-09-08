// Picker rows in src/lib/models.ts. The regression this guards: the host
// reports zero models for agents whose CLI has no list command (claude answers
// `providerHint: "anthropic"` and an empty list), so reading the raw discovery
// left the Model sheet with nothing but its own "Default model" row.

import { buildCatalog } from "@desktop/data/modelCatalog/build";
import type { ModelsDevIndex } from "@desktop/data/modelCatalog/modelsDev";
import type { AgentModels } from "@desktop/data/modelCatalog/types";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { loadModels, modelsFor } from "../src/lib/models";

const mocks = vi.hoisted(() => ({
  refreshCatalog: vi.fn(),
  loadCachedCatalog: vi.fn(() => ({ byId: {}, byAgent: {} })),
}));

vi.mock("@desktop/data/modelCatalog", () => ({
  loadCachedCatalog: mocks.loadCachedCatalog,
  refreshCatalog: mocks.refreshCatalog,
}));

/** Whether the call at `index` asked for a forced rebuild. */
const forced = (index: number) => mocks.refreshCatalog.mock.calls[index][1];

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

  beforeEach(() => {
    localStorage.clear();
    mocks.refreshCatalog.mockReset();
    mocks.refreshCatalog.mockResolvedValue(catalog);
  });

  it("rebuilds for a host the cache wasn't built from", async () => {
    // Half the catalog is the host's installed CLIs, so a phone re-paired to
    // another Mac must not serve the previous host's lists for up to an hour.
    await loadModels("host-a");
    await loadModels("host-b");

    expect(forced(1)).toBe(true);
  });

  it("serves the cache while the host is unchanged", async () => {
    await loadModels("host-a");
    await loadModels("host-a");

    expect(forced(1)).toBe(false);
  });

  it("does not claim the cache belongs to a host after a failed rebuild", async () => {
    mocks.refreshCatalog.mockResolvedValue(null);
    expect(await loadModels("host-a")).toBeNull();

    mocks.refreshCatalog.mockResolvedValue(catalog);
    await loadModels("host-a");

    expect(forced(1)).toBe(true);
  });
});
