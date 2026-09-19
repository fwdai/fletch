// ToolRow answers a sidebar `chatFocus` request by opening, scrolling into
// view on the next frame, and consuming the request. The repo has no DOM test
// environment, so the effect body lives in revealToolRow and is driven here
// with a fake frame scheduler and the real ui slice — the regression this
// guards is a clear issued before the frame, which re-ran the effect and let
// its cleanup cancel the scroll.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";
import { revealToolRow } from "@/components/Workspace/messages/revealToolRow";
import type { AppState } from "@/store/types";
import { createUiSlice } from "@/store/ui";

const makeStore = () => create<AppState>()((...a) => ({ ...createUiSlice(...a) }) as AppState);

/** A frame scheduler under test control: `flush` runs what is pending, and a
 *  cancelled id never runs. */
function fakeFrames() {
  const pending = new Map<number, () => void>();
  let next = 1;
  return {
    raf: (cb: () => void) => {
      const id = next++;
      pending.set(id, cb);
      return id;
    },
    caf: (id: number) => {
      pending.delete(id);
    },
    flush: () => {
      for (const [id, cb] of pending) {
        pending.delete(id);
        cb();
      }
    },
    size: () => pending.size,
  };
}

describe("revealToolRow", () => {
  it("opens at once, scrolls on the next frame, and only then clears the focus", () => {
    const store = makeStore();
    store.setState({ chatFocus: { agentId: "a1", toolUseId: "toolu_1" } });
    const frames = fakeFrames();
    const root = { scrollIntoView: vi.fn() };
    const open = vi.fn();

    revealToolRow(() => root, open, store.getState().clearChatFocus, frames.raf, frames.caf);

    // Synchronously: the row is open, nothing has scrolled, the request stands.
    expect(open).toHaveBeenCalledTimes(1);
    expect(root.scrollIntoView).not.toHaveBeenCalled();
    expect(store.getState().chatFocus).toEqual({ agentId: "a1", toolUseId: "toolu_1" });

    frames.flush();
    expect(root.scrollIntoView).toHaveBeenCalledWith({ block: "center" });
    expect(store.getState().chatFocus).toBeNull();
  });

  it("the cleanup cancels a frame that has not fired, and is harmless after it has", () => {
    const store = makeStore();
    store.setState({ chatFocus: { agentId: "a1", toolUseId: "toolu_1" } });
    const frames = fakeFrames();
    const root = { scrollIntoView: vi.fn() };

    // Unmounted before the frame: no scroll, and the request stays for the
    // row that mounts next (e.g. after a lazy history load).
    const cancel = revealToolRow(
      () => root,
      () => {},
      store.getState().clearChatFocus,
      frames.raf,
      frames.caf,
    );
    cancel();
    frames.flush();
    expect(root.scrollIntoView).not.toHaveBeenCalled();
    expect(store.getState().chatFocus).not.toBeNull();

    // Frame fired first: the later cleanup (the effect re-running once the
    // clear flips `focused`) has nothing left to cancel.
    const cancelAfter = revealToolRow(
      () => root,
      () => {},
      store.getState().clearChatFocus,
      frames.raf,
      frames.caf,
    );
    frames.flush();
    expect(root.scrollIntoView).toHaveBeenCalledTimes(1);
    expect(store.getState().chatFocus).toBeNull();
    cancelAfter();
    expect(frames.size()).toBe(0);
  });

  it("still consumes the request when the row is gone by the time the frame fires", () => {
    const store = makeStore();
    store.setState({ chatFocus: { agentId: "a1", toolUseId: "toolu_1" } });
    const frames = fakeFrames();

    revealToolRow(
      () => null,
      () => {},
      store.getState().clearChatFocus,
      frames.raf,
      frames.caf,
    );
    expect(() => frames.flush()).not.toThrow();
    expect(store.getState().chatFocus).toBeNull();
  });
});
