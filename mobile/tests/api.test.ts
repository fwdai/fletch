// The API facade is where a call is known to be a read or a write, and that
// is what decides whether it may be abandoned by a timer (see
// `CallOptions.timeoutMs`): a write the client gave up on is still running on
// the Mac.

import { describe, expect, it, vi } from "vitest";
import { createApi } from "../src/api";
import { READ_TIMEOUT_MS, type RemoteClient } from "../src/remote";

describe("api facade", () => {
  const client = { call: vi.fn(async () => null) } as unknown as RemoteClient;
  const api = createApi(client);
  const optionsOf = (op: string) =>
    (client.call as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === op)?.[2];

  it("bounds reads with the read timeout", async () => {
    await api.getWorkspace();
    await api.readSessionRecords("a");
    await api.getPrLive("a");
    await api.listProviderAccounts();
    await api.getProviderLimits("claude");
    for (const op of [
      "get_workspace",
      "read_session_records",
      "get_pr_live",
      "list_provider_accounts",
      "get_provider_limits",
    ]) {
      expect(optionsOf(op)).toEqual({ timeoutMs: READ_TIMEOUT_MS });
    }
  });

  it("leaves writes unbounded, however long they run", async () => {
    await api.syncSession("a");
    await api.pushAgent("a");
    await api.archiveAgent("a");
    await api.sendUserMessage("a", "t", "hi");
    await api.setActiveProviderAccount("claude", "work");
    // A refresh stores its reading on the host whether or not the phone waits.
    await api.refreshProviderLimits("claude", "work");
    // A switch relaunches the agent on the host whether or not the phone waits.
    await api.switchAgentAccount("a", "work");
    for (const op of [
      "sync_session",
      "push_agent",
      "archive_agent",
      "send_user_message",
      "set_active_provider_account",
      "refresh_provider_limits",
      "switch_agent_account",
    ]) {
      expect(optionsOf(op)).toBeUndefined();
    }
  });

  it("sends a per-agent switch with the desktop command's argument keys", async () => {
    await api.switchAgentAccount("a", "default");
    expect(client.call).toHaveBeenLastCalledWith("switch_agent_account", {
      agentId: "a",
      account: "default",
    });
  });
});
