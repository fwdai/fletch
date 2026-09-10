// The bug this pins: `spawnFromDraft` seeded its optimistic first bubble without
// the `turnId` it then sent the prompt under. The host announces every accepted
// message to every client (`turn:sent`), and the mirror that folds it in dedupes
// *by turn id only* — so the sender's own first prompt came back as a second,
// visually identical bubble. Launching an agent by typing a prompt was the only
// path with this flaw: the composer's `sendUserMessage` has always stamped the id.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { spawnAgent, sendUserMessage, getWorkspace } = vi.hoisted(() => ({
  spawnAgent: vi.fn(),
  sendUserMessage: vi.fn(),
  getWorkspace: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { spawnAgent, sendUserMessage, getWorkspace } }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));
vi.mock("@/data/slashCommands", () => ({ discoverCommands: vi.fn() }));

import type { ChatItem } from "@/adapters";
import type { TurnSentEvent } from "@/api";
import { mirrorSentTurn } from "@/helpers";
import { createDraftsSlice } from "./drafts";
import type { AppState } from "./types";

const PROMPT = "reorganize the settings pane";

const makeStore = () => {
  const store = create<AppState>()((...a) => ({ ...createDraftsSlice(...a) }) as AppState);
  store.setState({
    drafts: [
      {
        id: "d1",
        repoPath: "/repos/app",
        name: "chimborazo",
        provider: "claude",
        base: "main",
      },
    ],
    composerDrafts: {},
    customAgents: [],
    modelsByAgent: {},
    skills: [],
    managedLogs: {},
    managedBusy: {},
    // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  } as any);
  return store;
};

/** The `turn:sent` the host emits for the prompt the store just sent. At spawn
 *  the agent is still `Spawning`, which the backend reports as busy — hence
 *  `follow_up: true`, the variant that renders as a badge-less user bubble. */
const hostEcho = (turnId: string): TurnSentEvent => ({
  agent_id: "a1",
  turn_id: turnId,
  text: PROMPT,
  attachments: [],
  follow_up: true,
});

describe("spawning an agent from a typed prompt", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    spawnAgent.mockResolvedValue({ id: "a1" });
    sendUserMessage.mockResolvedValue(false);
    getWorkspace.mockResolvedValue(null);
  });

  it("seeds the first bubble under the same turn id it sends the prompt with", async () => {
    const store = makeStore();

    await store.getState().spawnFromDraft("d1", PROMPT, "claude", undefined);

    const [, sentTurnId, sentText] = sendUserMessage.mock.calls[0];
    expect(sentText).toBe(PROMPT);
    expect(store.getState().managedLogs.a1).toEqual([
      { kind: "user_message", text: PROMPT, turnId: sentTurnId },
    ]);
  });

  it("shows one bubble, not two, once the host echoes the send back", async () => {
    const store = makeStore();

    await store.getState().spawnFromDraft("d1", PROMPT, "claude", undefined);

    const log = store.getState().managedLogs.a1 as ChatItem[];
    const turnId = sendUserMessage.mock.calls[0][1] as string;
    // Same reference back = the mirror recognized the bubble as ours.
    expect(mirrorSentTurn(log, hostEcho(turnId))).toBe(log);
  });

  it("still mirrors a prompt this client didn't send (a phone, a git action)", async () => {
    const store = makeStore();

    await store.getState().spawnFromDraft("d1", PROMPT, "claude", undefined);

    const log = store.getState().managedLogs.a1 as ChatItem[];
    expect(mirrorSentTurn(log, hostEcho("someone-elses-turn"))).toHaveLength(log.length + 1);
  });
});
