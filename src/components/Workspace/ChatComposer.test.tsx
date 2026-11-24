// @vitest-environment happy-dom
// What a workspace's composer is handed for where it runs: a box at a cloud
// provider is given no folder from this Mac and no switch for a network that
// is already its machine's own; a box on this Mac keeps both.

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentRecord } from "@/api";
import { useAppStore } from "@/store";
import { render } from "@/test/render";
import { ChatComposer } from "./ChatComposer";

const { composer } = vi.hoisted(() => ({
  composer: vi.fn((_props: { folders?: boolean; network?: { open: boolean } }) => null),
}));
vi.mock("@/components/Composer", () => ({ Composer: composer }));
vi.mock("@/components/Composer/EnvironmentFrame", () => ({
  EnvironmentFrame: ({ children }: { children: unknown }) => children,
}));
vi.mock("@/storage/projectSettings", () => ({ getLinearTeamId: () => Promise.resolve(undefined) }));

const agent = (over: Partial<AgentRecord> = {}): AgentRecord => ({
  id: "a1",
  project_id: "p1",
  name: "moravia",
  provider: "claude",
  repos: [{ repo_path: "/repos/acme", subdir: "" }],
  task: "",
  status: "idle",
  created_at: "2026-11-19T00:00:00Z",
  sandbox_engine: "box",
  ...over,
});

/** The props the composer was last handed for `record`, its network open
 *  for this turn. */
function handed(record: AgentRecord) {
  useAppStore.setState({
    boxStates: { a1: { agent_id: "a1", phase: "running", turn_open: true } },
  });
  render(
    <ChatComposer
      agent={record}
      activeModel={undefined}
      liveBusy={false}
      liveStartedAt={undefined}
    />,
  );
  return composer.mock.lastCall?.[0];
}

beforeEach(() => {
  composer.mockClear();
  useAppStore.setState(useAppStore.getInitialState(), true);
});

describe("ChatComposer", () => {
  it("gives a box on this Mac its folders and its network switch", () => {
    const props = handed(agent());
    expect(props?.folders).toBe(true);
    expect(props?.network).toMatchObject({ open: true });
  });

  it("gives a box at a cloud provider neither", () => {
    const props = handed(agent({ cloud_provider: "fly" }));
    expect(props?.folders).toBe(false);
    expect(props?.network).toBeUndefined();
  });
});
