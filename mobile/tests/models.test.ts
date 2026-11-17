// Picker rows in src/lib/models.ts. The regression this guards: the host
// reports zero models for agents whose CLI has no list command (claude answers
// `providerHint: "anthropic"` and an empty list), so reading the raw discovery
// left the Model sheet with nothing but its own "Default model" row.

import { CATALOG_CACHE_KEY } from "@desktop/data/modelCatalog";
import { buildCatalog } from "@desktop/data/modelCatalog/build";
import type { ModelsDevIndex } from "@desktop/data/modelCatalog/modelsDev";
import type { AgentModels } from "@desktop/data/modelCatalog/types";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type KeyedLink, linkOf } from "../src/host";
import { forgetModels, loadModels, modelsFor, modelsOf } from "../src/lib/models";
import { deferred } from "./support";

const mocks = vi.hoisted(() => {
  // What an earlier build left in storage, before lib/models is first read.
  localStorage.setItem("modelCatalog.byHost", "{}");
  return { fetchModelsDevIndex: vi.fn() };
});

vi.mock("@desktop/data/modelCatalog/modelsDev", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@desktop/data/modelCatalog/modelsDev")>()),
  fetchModelsDevIndex: mocks.fetchModelsDevIndex,
}));

/** The desktop catalog's expiry. */
const HOUR = 60 * 60 * 1000;

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
  /** What a host with claude installed reports, and one with only codex. */
  const CLAUDE: AgentModels[] = [{ agent: "claude", providerHint: "anthropic", models: [] }];
  const CODEX: AgentModels[] = [{ agent: "codex", models: [{ id: "gpt-9", name: "GPT 9" }] }];
  const listsOf = (agents: AgentModels[]) => buildCatalog(agents, index).byAgent;

  /** `link`'s discovery answering `agents` — how a rebuild for its host is
   *  counted. */
  const discovering = (link: KeyedLink, agents: AgentModels[]) =>
    vi.spyOn(link.api, "discoverSupportedModels").mockResolvedValue(agents);

  beforeEach(() => {
    mocks.fetchModelsDevIndex.mockReset();
    mocks.fetchModelsDevIndex.mockResolvedValue(index);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("builds each host a catalog of its own, from that host's discovery", async () => {
    // Half a catalog is the host's installed CLIs, so one host must never be
    // served another's lists.
    const a = freshHost();
    const b = freshHost();
    const onA = discovering(a, CLAUDE);
    const onB = discovering(b, CODEX);

    expect(await loadModels(a)).toEqual(listsOf(CLAUDE));
    expect(await loadModels(b)).toEqual(listsOf(CODEX));
    expect(modelsOf(a.hostKey)).toEqual(listsOf(CLAUDE));
    expect(modelsOf(b.hostKey)).toEqual(listsOf(CODEX));
    expect(onA).toHaveBeenCalledOnce();
    expect(onB).toHaveBeenCalledOnce();
  });

  it("serves a host its own catalog again, however often the other host builds", async () => {
    const a = freshHost();
    const b = freshHost();
    const onA = discovering(a, CLAUDE);
    const onB = discovering(b, CODEX);
    await loadModels(a);
    await loadModels(b);
    await loadModels(a);
    await loadModels(b);
    // One rebuild each: going back and forth between hosts rebuilds nothing.
    expect(onA).toHaveBeenCalledOnce();
    expect(onB).toHaveBeenCalledOnce();
    // Kept as the desktop keeps its own, under a key of the host's.
    const kept = JSON.parse(localStorage.getItem(`${CATALOG_CACHE_KEY}:${b.hostKey}`) ?? "{}");
    expect(kept.catalog.byAgent).toEqual(listsOf(CODEX));
  });

  it("rebuilds a host's catalog once it has aged out, and only that host's", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const a = freshHost();
    const b = freshHost();
    const onA = discovering(a, CLAUDE);
    const onB = discovering(b, CODEX);
    await loadModels(a);
    vi.advanceTimersByTime(HOUR / 2);
    await loadModels(b);
    vi.advanceTimersByTime(HOUR / 2 + 1);

    await loadModels(a);
    await loadModels(b);
    expect(onA).toHaveBeenCalledTimes(2);
    expect(onB).toHaveBeenCalledOnce();
  });

  it("shares one rebuild between two pickers on the same host, not across hosts", async () => {
    const a = freshHost();
    const b = freshHost();
    const answer = deferred<AgentModels[]>();
    const onA = vi.spyOn(a.api, "discoverSupportedModels").mockReturnValue(answer.promise);
    const onB = discovering(b, CODEX);

    const first = loadModels(a);
    const second = loadModels(a);
    // Another host's rebuild is its own, and lands while host a's is out.
    expect(await loadModels(b)).toEqual(listsOf(CODEX));
    expect(onA).toHaveBeenCalledOnce();
    expect(onB).toHaveBeenCalledOnce();

    answer.resolve(CLAUDE);
    expect(await first).toEqual(listsOf(CLAUDE));
    expect(await second).toEqual(listsOf(CLAUDE));
    expect(modelsOf(b.hostKey)).toEqual(listsOf(CODEX));
  });

  it("reads models.dev once for every host within an expiry, and again after it", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    // Well past whatever an earlier case read: one of them reads an hour
    // into its own clock, and this clock starts over from the real time.
    vi.advanceTimersByTime(3 * HOUR);
    const a = freshHost();
    const b = freshHost();
    discovering(a, CLAUDE);
    discovering(b, CODEX);
    await loadModels(a);
    await loadModels(b);
    expect(mocks.fetchModelsDevIndex).toHaveBeenCalledOnce();

    vi.advanceTimersByTime(HOUR + 1);
    await loadModels(a);
    expect(mocks.fetchModelsDevIndex).toHaveBeenCalledTimes(2);
  });

  it("keeps nothing for a host whose rebuild failed, and tries again next time", async () => {
    const a = freshHost();
    // A host whose CLIs report nothing builds an empty catalog: a failure.
    const onA = discovering(a, []);
    expect(await loadModels(a)).toBeNull();
    expect(modelsOf(a.hostKey)).toEqual({});

    onA.mockResolvedValue(CLAUDE);
    expect(await loadModels(a)).toEqual(listsOf(CLAUDE));
    expect(onA).toHaveBeenCalledTimes(2);
  });

  it("forgets a host's catalog, and nothing of another's", async () => {
    const a = freshHost();
    const b = freshHost();
    discovering(a, CLAUDE);
    discovering(b, CODEX);
    await loadModels(a);
    await loadModels(b);
    forgetModels(a.hostKey);
    expect(modelsOf(a.hostKey)).toEqual({});
    expect(modelsOf(b.hostKey)).toEqual(listsOf(CODEX));
  });

  it("clears what earlier builds kept, which nothing reads", () => {
    expect(localStorage.getItem("modelCatalog.byHost")).toBeNull();
  });
});
