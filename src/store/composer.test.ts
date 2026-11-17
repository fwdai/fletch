import { describe, expect, it } from "vitest";
import { create } from "zustand";
import { activeEngine } from "@/api/engine";
import { createComposerSlice } from "./composer";
import type { AppState } from "./types";

const makeStore = () =>
  create<AppState>()(
    (set, get) => ({ ...createComposerSlice(set, get, activeEngine) }) as AppState,
  );

describe("composer roadmap picks", () => {
  it("forgets a pick once the send that carried it is done", () => {
    const store = makeStore();
    store.getState().setComposerRoadmapPick("fuji", { id: "item-1", code: "FLT-1" });
    store.getState().clearComposerRoadmapPick("fuji", "item-1");
    expect(store.getState().composerRoadmapPicks).toEqual({});
  });

  it("keeps a pick made while an earlier send was still in flight", () => {
    const store = makeStore();
    // Send A carried item-1; before it finished, item-2 was picked into the
    // next draft. A finishing must not take item-2 with it.
    store.getState().setComposerRoadmapPick("fuji", { id: "item-1", code: "FLT-1" });
    store.getState().setComposerRoadmapPick("fuji", { id: "item-2", code: "FLT-2" });
    store.getState().clearComposerRoadmapPick("fuji", "item-1");
    expect(store.getState().composerRoadmapPicks).toEqual({
      fuji: { id: "item-2", code: "FLT-2" },
    });
  });

  it("leaves other agents' picks alone", () => {
    const store = makeStore();
    store.getState().setComposerRoadmapPick("fuji", { id: "item-1", code: "FLT-1" });
    store.getState().setComposerRoadmapPick("alps", { id: "item-1", code: "FLT-1" });
    store.getState().clearComposerRoadmapPick("fuji", "item-1");
    expect(store.getState().composerRoadmapPicks).toEqual({
      alps: { id: "item-1", code: "FLT-1" },
    });
  });
});
