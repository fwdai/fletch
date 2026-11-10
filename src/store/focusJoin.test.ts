// The request the add-project flows leave for the sidebar: which project is
// waiting to join the focus, and what withdraws it.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";

vi.mock("@/api", () => ({ api: {} }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import type { AppState } from "./types";
import { createUiSlice } from "./ui";

const makeStore = (projectFocus: string[]) => {
  const store = create<AppState>()((...a) => ({ ...createUiSlice(...a) }) as unknown as AppState);
  store.setState({ projectFocus });
  return store;
};

describe("focusJoin", () => {
  it("is asked for only while something is saved as the focus", () => {
    const all = makeStore([]);
    all.getState().requestFocusJoin("p-new");
    expect(all.getState().focusJoin).toBeNull();

    const narrowed = makeStore(["p-a"]);
    narrowed.getState().requestFocusJoin("p-new");
    expect(narrowed.getState().focusJoin).toBe("p-new");
  });

  it("is withdrawn by any change to the focus", () => {
    const store = makeStore(["p-a"]);
    store.getState().requestFocusJoin("p-new");
    store.getState().setProjectFocus(["p-b"]);
    expect(store.getState().focusJoin).toBeNull();
    expect(store.getState().projectFocus).toEqual(["p-b"]);
  });
});
