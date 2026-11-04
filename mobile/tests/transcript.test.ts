import { liveActivity } from "@desktop/components/Workspace/messages/steps";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Transcript } from "../src/screens/Agent/Transcript";

vi.mock("@desktop/components/Workspace/messages/steps", async (importOriginal) => {
  const real =
    await importOriginal<typeof import("@desktop/components/Workspace/messages/steps")>();
  return { ...real, liveActivity: vi.fn(real.liveActivity) };
});

type Items = Parameters<typeof Transcript>[0]["items"];

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const read = (id: string): Items[number] => ({
  kind: "tool_pair",
  call: { kind: "tool_call", id, name: "Read", input: { file_path: `/${id}.ts` } },
  result: { kind: "tool_result", tool_use_id: id, content: "x" },
});
const prompt = (text: string, ended: boolean): Items[number] => ({
  kind: "user_message",
  text,
  startedAt: 0,
  endedAt: ended ? 141_000 : undefined,
});
const say = (text: string): Items[number] => ({ kind: "agent_message", text });
const streaming = (text: string): Items[number] => ({
  kind: "agent_message",
  text,
  streaming: true,
});
const hook: Items[number] = { kind: "notice", subtype: "hook_output", text: "lint ok" };

describe("Transcript", () => {
  let host: HTMLDivElement;
  let root: Root;
  const render = (items: Items, busy = false) =>
    act(() => root.render(createElement(Transcript, { agentId: "a", items, busy })));

  beforeEach(() => {
    host = document.createElement("div");
    root = createRoot(host);
    vi.mocked(liveActivity).mockClear();
  });
  afterEach(() => act(() => root.unmount()));

  // The case that hid an answer: it's followed by a tool call and another line.
  it("keeps a settled turn's words on the page and closes it on its run time", () => {
    render([
      prompt("audit this", true),
      say("the assessment"),
      read("wake"),
      say("nothing further"),
      prompt("next", false),
    ]);
    expect(host.textContent).toContain("the assessment");
    expect(host.textContent).toContain("nothing further");
    expect(host.querySelectorAll(".steprun")).toHaveLength(1);
    expect(host.querySelector(".turn-ran")?.textContent).toBe("Ran 2m 21s");
  });

  it("holds the footer back while the agent is on that turn", () => {
    const items = [prompt("go", true), say("working")];
    render(items, true);
    expect(host.querySelector(".turn-ran")).toBeNull();
    render(items, false);
    expect(host.querySelector(".turn-ran")).not.toBeNull();
  });

  it("opens a run to steps the app draws itself", () => {
    render([prompt("go", true), read("a"), hook, say("done")]);
    expect(host.textContent).not.toContain("lint ok");
    const head = host.querySelector<HTMLButtonElement>(".steprun-head");
    act(() => head?.click());
    expect(host.textContent).toContain("lint ok");
  });

  // Every settled run is on the page now, so each one has to sit out the
  // stream: a delta re-renders only what it changed.
  it("leaves settled runs alone as the live turn streams", () => {
    const settled = [prompt("one", true), read("a"), say("done"), prompt("two", false)];
    render([...settled, read("b"), streaming("wor")], true);
    const live = read("b");
    render([...settled, live, streaming("wor")], true);
    vi.mocked(liveActivity).mockClear();

    render([...settled, live, streaming("working")], true);
    expect(liveActivity).not.toHaveBeenCalled();
  });
});
