import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { expect, test, vi } from "vitest";
import { ProviderMark } from "../src/components/ui";

const CLAUDE_SVG = `<svg viewBox="0 0 24 24"><script>bad()</script><path fill="currentColor" d="M1 1h2v2z"/></svg>`;

test("inlines the brand svg and strips scripts", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => {
      expect(url).toBe("https://fletch.sh/agents/claude.svg");
      return { ok: true, text: async () => CLAUDE_SVG } as Response;
    }),
  );
  const host = document.createElement("div");
  document.body.append(host);
  await act(async () => {
    createRoot(host).render(createElement(ProviderMark, { id: "claude" }));
  });
  expect(host.querySelector(".pm .pm-svg svg")).not.toBeNull();
  expect(host.innerHTML).not.toContain("<script>");
  expect(host.textContent).toBe("");
});

test("falls back to the monogram when the icon is missing", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({ ok: false, status: 404 }) as Response),
  );
  const host = document.createElement("div");
  document.body.append(host);
  await act(async () => {
    createRoot(host).render(createElement(ProviderMark, { id: "codex" }));
  });
  expect(host.querySelector("svg")).toBeNull();
  expect(host.textContent).toBe("CX");
});
