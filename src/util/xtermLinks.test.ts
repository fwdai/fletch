import { beforeEach, describe, expect, it, vi } from "vitest";

const { open } = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open }));

import { hyperlinkHandler } from "./xtermLinks";

const URL = "https://claude.ai/oauth/authorize?code=true&state=abc";
const range = { start: { x: 1, y: 1 }, end: { x: 10, y: 2 } };

beforeEach(() => {
  open.mockReset();
  open.mockResolvedValue(undefined);
});

describe("hyperlinkHandler", () => {
  it("opens the hyperlink target in the system browser", () => {
    hyperlinkHandler.activate({} as MouseEvent, URL, range);
    expect(open).toHaveBeenCalledWith(URL);
  });

  it("does not throw when the opener rejects", async () => {
    open.mockRejectedValue(new Error("not allowed"));
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    hyperlinkHandler.activate({} as MouseEvent, URL, range);
    await Promise.resolve();
    expect(error).toHaveBeenCalled();
    error.mockRestore();
  });
});
