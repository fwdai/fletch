// @vitest-environment happy-dom
import { describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@/test/render";
import { useComposerInput } from "./useComposerInput";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-fs", () => ({ stat: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));

const CHIP = { path: "/Users/me/notes", writable: false };

describe("a composer's folder chips", () => {
  it("are neither shown nor sent once folders can't be attached, and return when they can", async () => {
    const { result, rerender } = renderHook(
      ({ folders }: { folders: boolean }) => useComposerInput({ provider: "claude", folders }),
      { initialProps: { folders: true } },
    );
    act(() => result.current.restore({ text: "look here", attachments: [], folders: [CHIP] }));
    expect(result.current.folders).toEqual([CHIP]);

    // The draft's workspace type becomes one that mounts no folder.
    rerender({ folders: false });
    expect(result.current.folders).toEqual([]);
    expect(await result.current.foldersToSend("look here", result.current.folders)).toEqual([]);

    rerender({ folders: true });
    expect(result.current.folders).toEqual([CHIP]);
  });
});
