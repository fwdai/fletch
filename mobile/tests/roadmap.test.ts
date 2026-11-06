// The project screen's Roadmap segment end to end over the mock host: the real
// protocol client, the real store slice, the real pane. The jsdom URL in
// vite.config.ts puts this file in mock mode (see `mockEnabled`).

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { roadmapLists } from "../src/lib/roadmap";
import { roadmapItem } from "../src/remote/mock/fixtures";
import { RoadmapPane } from "../src/screens/Project/Roadmap";
import { api, client, useStore } from "../src/store";

const state = () => useStore.getState();
const PROJECT = "prj-fletch";

/** Deliver a host event frame to the store's listeners, as the socket would. */
const hostEvent = (event: string, payload: unknown) =>
  (
    client as unknown as { dispatchEvent(frame: { event: string; payload: unknown }): void }
  ).dispatchEvent({ event, payload });

const lists = () => {
  const board = state().boards[PROJECT];
  if (!board) throw new Error("the board was never read");
  return roadmapLists(board);
};
const chipOf = (code: string) => lists().inbox.find((e) => e.item.code === code)?.chip;

beforeAll(async () => {
  await state().init();
  await vi.waitFor(() => expect(state().connection).toBe("connected"), { timeout: 5000 });
});

describe("the roadmap segment's lists", () => {
  it("lists the Inbox with the desktop's chips, and Next up in board order", async () => {
    const row = (code: string, i: Partial<Parameters<typeof roadmapItem>[0]>) =>
      roadmapItem({ id: code, code, title: code, why: "", status: "open", ...i });
    const read = vi.spyOn(api, "roadmapListItems").mockResolvedValueOnce([
      row("FLT-1", { horizon: "later" }),
      row("FLT-2", { rank: 2000 }),
      row("FLT-3", { status: "proposed", rank: 100 }),
      row("FLT-4", { status: "queued", rank: 500 }),
      row("FLT-5", { status: "active", rank: 50 }),
      row("FLT-6", { status: "captured", horizon: "later" }),
      row("FLT-7", {
        status: "captured",
        kind: "idea",
        triage: { brief: "Worth a look", questions: ["Who?", "When?"] },
      }),
    ]);
    try {
      await state().loadBoard(PROJECT);
    } finally {
      read.mockRestore();
    }
    // Only open and queued rows of the Next horizon, by rank: no ghost, nothing
    // already dispatched, nothing from another horizon.
    expect(lists().next.map((i) => i.code)).toEqual(["FLT-4", "FLT-2"]);
    expect(lists().inbox.map((e) => [e.item.code, e.chip])).toEqual([
      ["FLT-6", "Triaging…"],
      ["FLT-7", "Idea · brief, 2 questions"],
    ]);
  });

  it("follows the triage dial and the drainer's word on a waiting capture", async () => {
    await state().loadBoard(PROJECT);
    expect(chipOf("FLT-211")).toBe("Triaging…");
    hostEvent("project_settings:changed", {
      project_id: PROJECT,
      key: "roadmap.triage",
      value: "0",
    });
    expect(chipOf("FLT-211")).toBe("Untriaged");
    hostEvent("project_settings:changed", {
      project_id: PROJECT,
      key: "roadmap.triage",
      value: null,
    });
    hostEvent("roadmap:item-event", {
      id: "evt-1",
      item_id: "itm-run-port",
      project_id: PROJECT,
      actor: "drainer",
      kind: "note",
      detail: "Waiting for a free PM slot",
      created_at: Date.now(),
    });
    expect(chipOf("FLT-211")).toBe("Waiting for a PM chat");
  });

  it("follows `roadmap:item` into Next up and `roadmap:item-deleted` off the board", async () => {
    await state().loadBoard(PROJECT);
    expect(lists().next.map((i) => i.code)).toEqual(["FLT-205"]);
    // Accepted on the Mac: the ghost becomes an open row in Next.
    await api.roadmapUpdateItem("itm-offline-queue", { status: "open" });
    await vi.waitFor(() => expect(lists().next.map((i) => i.code)).toEqual(["FLT-205", "FLT-207"]));
    hostEvent("roadmap:item-deleted", "itm-held-sends");
    expect(lists().next.map((i) => i.code)).toEqual(["FLT-207"]);
  });
});

describe("capturing from the phone", () => {
  /** Render the pane, type `text` into its field, and tap Capture. */
  const captureThrough = async (text: string) => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    await act(async () => root.render(createElement(RoadmapPane, { projectId: PROJECT })));
    const field = host.querySelector("textarea");
    if (!field) throw new Error("no capture field");
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(
        field,
        text,
      );
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const button = [...host.querySelectorAll("button")].find((b) => b.textContent === "Capture");
    await act(async () => button?.click());
    return {
      field,
      host,
      unmount: () => {
        act(() => root.unmount());
        host.remove();
      },
    };
  };

  it("lands the note in the Inbox as the host splits it, and clears the field", async () => {
    const capture = vi.spyOn(api, "roadmapCapture");
    const { field, host, unmount } = await captureThrough(
      "Remember the simulator\nIt boots a fresh one every run.",
    );
    try {
      expect(capture).toHaveBeenCalledWith(
        PROJECT,
        "Remember the simulator\nIt boots a fresh one every run.",
      );
      await vi.waitFor(() => expect(field.value).toBe(""));
      const landed = lists().inbox.find((e) => e.item.title === "Remember the simulator");
      expect(landed?.item.why).toBe("It boots a fresh one every run.");
      expect(host.textContent).toContain("Remember the simulator");
    } finally {
      capture.mockRestore();
      unmount();
    }
  });

  it("keeps the text, with the host's reason, when the capture is refused", async () => {
    const capture = vi
      .spyOn(api, "roadmapCapture")
      .mockRejectedValueOnce(new Error("device lacks the roadmap scope"));
    const { field, host, unmount } = await captureThrough("An idea that will not land");
    try {
      await vi.waitFor(() =>
        expect(host.querySelector(".notice-bar")?.textContent).toContain(
          "device lacks the roadmap scope",
        ),
      );
      expect(field.value).toBe("An idea that will not land");
    } finally {
      capture.mockRestore();
      unmount();
    }
  });
});
