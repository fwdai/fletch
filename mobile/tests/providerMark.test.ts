import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { expect, test, vi } from "vitest";
import { ProviderMark } from "../src/components/ui";
import { useProviderIcon } from "../src/lib/useProviderIcon";

const mark = (slug: string) => `<svg viewBox="0 0 24 24"><title>${slug}</title></svg>`;
const CLAUDE_SVG = `<svg viewBox="0 0 24 24"><script>bad()</script><path fill="currentColor" d="M1 1h2v2z"/></svg>`;

// The loader's cache lives for the lifetime of the module, so each test claims
// its own provider rather than fighting over one.
const mount = () => {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  return { host, root };
};

test("inlines the brand svg and strips scripts", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => {
      expect(url).toBe("https://fletch.sh/agents/claude.svg");
      return { ok: true, text: async () => CLAUDE_SVG } as Response;
    }),
  );
  const { host, root } = mount();
  await act(async () => {
    root.render(createElement(ProviderMark, { id: "claude" }));
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
  const { host, root } = mount();
  await act(async () => {
    root.render(createElement(ProviderMark, { id: "codex" }));
  });
  expect(host.querySelector("svg")).toBeNull();
  expect(host.textContent).toBe("CX");
});

test("marks for one provider share a single request", async () => {
  const fetchMock = vi.fn(async () => ({ ok: true, text: async () => mark("cursor") }) as Response);
  vi.stubGlobal("fetch", fetchMock);
  // What the agent screen does: the same mark in the header and the composer.
  const { host, root } = mount();
  await act(async () => {
    root.render(
      createElement("div", null, [
        createElement(ProviderMark, { id: "cursor", key: "head" }),
        createElement(ProviderMark, { id: "cursor", key: "composer", lg: true }),
      ]),
    );
  });
  expect(fetchMock).toHaveBeenCalledTimes(1);
  expect(host.querySelectorAll(".pm-svg svg")).toHaveLength(2);
});

test("switching provider never renders the previous brand", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) =>
      url.includes("opencode")
        ? ({ ok: true, text: async () => mark("opencode") } as Response)
        : // The incoming provider's icon stays in flight, so anything rendered
          // under it could only have come from the outgoing one.
          new Promise<Response>(() => {}),
    ),
  );

  const seen: (string | null)[] = [];
  function Probe({ slug }: { slug: string }) {
    seen.push(useProviderIcon(slug).svg);
    return null;
  }

  const { root } = mount();
  await act(async () => {
    root.render(createElement(Probe, { slug: "opencode" }));
  });
  expect(seen).toContain(mark("opencode"));

  seen.length = 0;
  await act(async () => {
    root.render(createElement(Probe, { slug: "pi" }));
  });
  expect(seen.length).toBeGreaterThan(0);
  expect(seen).not.toContain(mark("opencode"));
});

test("adopts a cache entry that landed after its render", async () => {
  // Effects flush after paint, so a sibling mark's shared request can fill the
  // cache in the window between this mark's render and its effect. The stub
  // reproduces that window exactly: empty at render time, warm afterwards.
  const AGY_SVG = mark("antigravity");
  let reads = 0;
  vi.resetModules();
  vi.doMock("@desktop/data/providerIcon", () => ({
    cachedProviderIcon: () => (reads++ === 0 ? null : AGY_SVG),
    loadProviderIcon: async () => AGY_SVG,
  }));
  const { useProviderIcon: hook } = await import("../src/lib/useProviderIcon");

  let svg: string | null = null;
  function Probe() {
    svg = hook("antigravity").svg;
    return null;
  }
  const { root } = mount();
  await act(async () => {
    root.render(createElement(Probe));
  });
  expect(svg).toBe(AGY_SVG);

  vi.doUnmock("@desktop/data/providerIcon");
  vi.resetModules();
});
