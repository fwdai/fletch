import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  discoverSupportedModels: vi.fn(),
  fetchModelsDevIndex: vi.fn(),
}));

vi.mock("@/api", () => ({
  api: {
    discoverSupportedModels: mocks.discoverSupportedModels,
  },
}));

vi.mock("@/data/modelCatalog/modelsDev", () => ({
  fetchModelsDevIndex: mocks.fetchModelsDevIndex,
}));

const CACHE_KEY = "modelCatalog.cache.v14";

const storage = new Map<string, string>();

beforeEach(() => {
  storage.clear();
  vi.resetModules();
  mocks.discoverSupportedModels.mockReset();
  mocks.fetchModelsDevIndex.mockReset();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => {
      storage.set(key, value);
    },
    removeItem: (key: string) => {
      storage.delete(key);
    },
    clear: () => {
      storage.clear();
    },
  });
});

describe("refreshCatalog", () => {
  it("keeps the last good cache when models.dev fails after discovery succeeds", async () => {
    storage.set(
      CACHE_KEY,
      JSON.stringify({
        builtAt: 1,
        catalog: {
          byId: {
            saved: {
              id: "saved",
              name: "Saved",
              contextWindow: 1,
              reasoning: false,
            },
          },
          byAgent: {},
        },
      }),
    );
    mocks.discoverSupportedModels.mockResolvedValue([{ agent: "codex", models: [] }]);
    mocks.fetchModelsDevIndex.mockResolvedValue(null);

    const { loadCachedCatalog, refreshCatalog } = await import("@/data/modelCatalog");
    const result = await refreshCatalog(true);

    expect(result).toBeNull();
    expect(loadCachedCatalog().byId.saved.id).toBe("saved");
    expect(storage.get(CACHE_KEY)).toContain("saved");
  });

  it("dedupes concurrent refreshes so only one rebuild runs", async () => {
    let resolveDiscover: (value: unknown) => void = () => {};
    let resolveIndex: (value: unknown) => void = () => {};
    const discover = new Promise((resolve) => {
      resolveDiscover = resolve;
    });
    const index = new Promise((resolve) => {
      resolveIndex = resolve;
    });

    mocks.discoverSupportedModels.mockReturnValue(discover);
    mocks.fetchModelsDevIndex.mockReturnValue(index);

    const { refreshCatalog } = await import("@/data/modelCatalog");
    const first = refreshCatalog(true);
    const second = refreshCatalog(true);

    expect(first).toBe(second);
    expect(mocks.discoverSupportedModels).toHaveBeenCalledTimes(1);
    expect(mocks.fetchModelsDevIndex).toHaveBeenCalledTimes(1);

    resolveDiscover([{ agent: "codex", models: [{ id: "gpt-5.5" }] }]);
    resolveIndex({
      byId: {
        "gpt-5.5": {
          id: "gpt-5.5",
          name: "GPT-5.5",
          contextWindow: 400_000,
          reasoning: true,
          releaseDate: "2025-12-11",
        },
      },
      byProvider: {
        openai: ["gpt-5.5"],
      },
    });

    const result = await first;
    expect(result?.byId["gpt-5.5"]?.name).toBe("GPT-5.5");
    expect(storage.get(CACHE_KEY)).toContain("gpt-5.5");
  });
});
