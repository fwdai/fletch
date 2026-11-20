// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import type { CheckoutFileContents } from "@/api";
import { type Engine, EngineContext } from "@/api/engine";
import { render, screen } from "@/test/render";
import { ChangedImage } from "./ChangedImage";

const contents = (over: Partial<CheckoutFileContents>): CheckoutFileContents => ({
  text: "",
  lang: "",
  status: "M",
  chg_add: [],
  chg_mod: [],
  binary: true,
  too_large: false,
  ...over,
});

function renderWith(read: () => Promise<CheckoutFileContents>) {
  const engine = { api: { readCheckoutFile: read } } as unknown as Engine;
  return render(
    <EngineContext.Provider value={engine}>
      <ChangedImage agentId="a1" path="docs/logo.png" base="head" rev="1:2">
        <div>No textual diff</div>
      </ChangedImage>
    </EngineContext.Provider>,
  );
}

describe("ChangedImage", () => {
  it("draws the file when it is an image", async () => {
    renderWith(async () => contents({ image: { mime: "image/png", data: "AAAA" } }));
    const img = await screen.findByRole("img", { name: "logo.png" });
    expect(img.getAttribute("src")).toBe("data:image/png;base64,AAAA");
    expect(screen.queryByText("No textual diff")).toBeNull();
  });

  it("keeps the diff's own message for any other file", async () => {
    let settled = false;
    renderWith(async () => {
      settled = true;
      return contents({});
    });
    await screen.findByText("No textual diff");
    expect(settled).toBe(true);
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("keeps the diff's own message when the file can't be read", async () => {
    renderWith(() => Promise.reject(new Error("gone")));
    await screen.findByText("No textual diff");
    expect(screen.queryByRole("img")).toBeNull();
  });
});
