// @vitest-environment happy-dom
// Escape has one owner at a time: the topmost dismissible surface, and the
// global "close the screen" action only when none is up. Pinned from the
// keypress down, since the bug this guards against is two listeners on one
// key each doing the right thing alone.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "@/store";
import { renderHook } from "@/test/render";
import { useGlobalShortcuts } from "@/util/shortcuts";
import { useEscape } from "./useEscape";

const pressEscape = () =>
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

describe("Escape", () => {
  beforeEach(() => {
    useAppStore.setState({ settingsScreenOpen: true, paletteOpen: false });
  });

  // The two listeners sit on the window in the order they were registered,
  // and either can be first: the shortcuts mount with the app, but the
  // surfaces' shared listener is re-added whenever the first one mounts.
  for (const first of ["shortcuts", "surface"] as const) {
    it(`closes the surface and leaves the screen under it (${first} listening first)`, () => {
      const onClose = vi.fn();
      const mounts = {
        shortcuts: () => renderHook(() => useGlobalShortcuts()),
        surface: () => renderHook(() => useEscape(onClose)),
      };
      const order =
        first === "shortcuts"
          ? [mounts.shortcuts, mounts.surface]
          : [mounts.surface, mounts.shortcuts];
      const [a, b] = order.map((mount) => mount());
      const surface = first === "shortcuts" ? b : a;

      pressEscape();
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(useAppStore.getState().settingsScreenOpen).toBe(true);

      // With the surface gone, the next press is the screen's.
      surface.unmount();
      pressEscape();
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(useAppStore.getState().settingsScreenOpen).toBe(false);
    });
  }

  it("goes to the topmost surface only", () => {
    const outer = vi.fn();
    const inner = vi.fn();
    renderHook(() => useEscape(outer));
    renderHook(() => useEscape(inner));
    pressEscape();
    expect(inner).toHaveBeenCalledTimes(1);
    expect(outer).not.toHaveBeenCalled();
  });
});
