// Moving one agent onto another account, end to end over the mock host: the
// real protocol client, the real store slice, the real More sheet and picker.
// The jsdom URL in vite.config.ts puts this file in mock mode (see
// `mockEnabled`). The mock refuses the way the engine does
// (`supervisor/account_switch.rs`), so the refusals here are the host's words.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { Api } from "../src/api";
import { MOCK_HOST_KEY } from "../src/remote/mock";
import { AccountPickerSheet } from "../src/sheets/AccountPickerSheet";
import { AgentMoreSheet } from "../src/sheets/AgentMoreSheet";
import { agentOf, appStore } from "../src/store";
import { withHost } from "../src/store/hostNav";
import { deferred, hostEvent, hostOnScreen, launchMock, onHost } from "./support";

const app = () => appStore.getState();
const host = () => hostOnScreen().getState();

/** The mock host's api: the one `init` pairs with and the store stays live on. */
let api: Api;

const stampOf = (id: string) => agentOf(host(), id)?.account;

beforeAll(async () => {
  ({ api } = await launchMock());
  await host().loadProviderAccounts();
});

describe("switchAgentAccount over the mock host", () => {
  it("restamps the agent with the account the host hands back", async () => {
    expect(stampOf("kamakura")).toBe("work");

    const record = await host().switchAgentAccount("kamakura", "default");

    // The host spells the CLI's own login as no stamp.
    expect(record?.account).toBeNull();
    expect(stampOf("kamakura")).toBeNull();
    expect(host().switchingAccount.kamakura).toBe(false);
    // Only this agent moved.
    expect(stampOf("arabia")).toBe("work");
  });

  it("refuses the account the agent already runs under", async () => {
    await expect(host().switchAgentAccount("kamakura", "default")).rejects.toThrow(
      "This agent already runs under the `default` account.",
    );
  });

  it("refuses a signed-out target and leaves the stamp alone", async () => {
    await expect(host().switchAgentAccount("caspian", "personal")).rejects.toThrow(
      "The `personal` account isn't signed in",
    );
    expect(stampOf("caspian")).toBeNull();
    expect(host().switchingAccount.caspian).toBe(false);
  });

  it("refuses mid-turn", async () => {
    // Held on a tool approval for the whole run, so it stays mid-turn.
    expect(agentOf(host(), "pamukkale")?.status).toBe("running");
    await expect(host().switchAgentAccount("pamukkale", "default")).rejects.toThrow(
      "Wait for the turn to finish before switching accounts.",
    );
    expect(stampOf("pamukkale")).toBe("work");
  });

  it("restamps a planning chat in the chat registry", async () => {
    await host().loadChats("prj-fletch");
    expect(stampOf("sakura")).toBe("work");

    await host().switchAgentAccount("sakura", "default");

    expect(host().chats["prj-fletch"].find((c) => c.id === "sakura")?.account).toBeNull();
  });

  it("keeps a chat that finished while its list was out finished", async () => {
    // The host answers the list off records read before the chat's turn
    // ended, and the status that ends it reaches the phone first.
    const list = api.listProjectChats;
    let release: () => void = () => {};
    vi.spyOn(api, "listProjectChats").mockImplementationOnce(async (projectId, purpose) => {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return (await list(projectId, purpose)).map((c) =>
        c.id === "sakura" ? { ...c, status: "running" as const } : c,
      );
    });
    const read = host().loadChats("prj-fletch");
    hostEvent("agent:status", { agent_id: "sakura", status: "idle" });
    release();
    await read;
    expect(host().chats["prj-fletch"].find((c) => c.id === "sakura")?.status).toBe("idle");
  });

  it("replays only its own project's patches over a list's answer", async () => {
    // Project A's list is out; meanwhile a chat in project B is patched and B
    // re-lists fresh. A's late answer must not put B's older patch back.
    const list = api.listProjectChats;
    let release: () => void = () => {};
    vi.spyOn(api, "listProjectChats").mockImplementationOnce(async (projectId, purpose) => {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return list(projectId, purpose);
    });
    const atlas = host().loadChats("prj-atlas");
    host().patchChat("sakura", { account: "stale" });
    await host().loadChats("prj-fletch");
    const fresh = host().chats["prj-fletch"].find((c) => c.id === "sakura")?.account;
    expect(fresh).not.toBe("stale");
    release();
    await atlas;
    expect(host().chats["prj-fletch"].find((c) => c.id === "sakura")?.account).toBe(fresh);
  });

  it("keeps a chat resolved by id while its project's list was out", async () => {
    // A notification tap resolves a chat the list (read before it existed)
    // does not hold; the late answer must not drop it.
    const list = api.listProjectChats;
    let release: () => void = () => {};
    vi.spyOn(api, "listProjectChats").mockImplementationOnce(async (projectId, purpose) => {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return list(projectId, purpose);
    });
    const sakura = host().chats["prj-fletch"].find((c) => c.id === "sakura");
    const found = { ...(sakura as NonNullable<typeof sakura>), id: "found-later", name: "found" };
    vi.spyOn(api, "getAgent").mockResolvedValueOnce(found);
    const read = host().loadChats("prj-fletch");
    await host().ensureAgent("found-later");
    expect(host().chats["prj-fletch"].some((c) => c.id === "found-later")).toBe(true);
    release();
    await read;
    expect(host().chats["prj-fletch"].some((c) => c.id === "found-later")).toBe(true);
    // Not a fixture of the mock host: the next real list drops it again.
    await host().loadChats("prj-fletch");
    expect(host().chats["prj-fletch"].some((c) => c.id === "found-later")).toBe(false);
  });

  it("lands only the newest of two chat lists in flight", async () => {
    const list = api.listProjectChats;
    let releaseFirst: () => void = () => {};
    vi.spyOn(api, "listProjectChats").mockImplementationOnce(async (projectId, purpose) => {
      await new Promise<void>((resolve) => {
        releaseFirst = resolve;
      });
      return (await list(projectId, purpose)).map((c) =>
        c.id === "sakura" ? { ...c, status: "running" as const } : c,
      );
    });
    const first = host().loadChats("prj-fletch");
    await host().loadChats("prj-fletch");
    releaseFirst();
    await first;
    expect(host().chats["prj-fletch"].find((c) => c.id === "sakura")?.status).toBe("idle");
  });

  it("holds only until the provider's selection next moves every agent", async () => {
    await host().setActiveProviderAccount("claude", "default");
    await host().setActiveProviderAccount("claude", "work");

    await vi.waitFor(() => expect(stampOf("kamakura")).toBe("work"));
    // A loaded planning chat follows too: it is absent from the snapshot the
    // radio's `workspace:changed` refreshes, so its own list is read again.
    await vi.waitFor(() =>
      expect(host().chats["prj-fletch"].find((c) => c.id === "sakura")?.account).toBe("work"),
    );
  });

  // Last: it deletes the chat on the mock host.
  it("keeps a chat deleted while its project's list was out deleted", async () => {
    const list = api.listProjectChats;
    let release: () => void = () => {};
    vi.spyOn(api, "listProjectChats").mockImplementationOnce(async (projectId, purpose) => {
      // Read before the delete: the answer still holds the chat.
      const rows = await list(projectId, purpose);
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return rows;
    });
    const read = host().loadChats("prj-fletch");
    await host().deleteChat("sakura");
    expect(host().chats["prj-fletch"].some((c) => c.id === "sakura")).toBe(false);
    release();
    await read;
    expect(host().chats["prj-fletch"].some((c) => c.id === "sakura")).toBe(false);
  });
});

describe("the More sheet's account row and its picker", () => {
  let host: HTMLDivElement;
  let root: Root;

  const render = async (agentId: string) => {
    await act(async () => {
      root.render(
        onHost(
          MOCK_HOST_KEY,
          createElement(
            "div",
            null,
            createElement(AgentMoreSheet, { open: true, onClose: () => {}, agentId }),
            createElement(AccountPickerSheet, {
              open: app().sheet?.name === "accountPicker" && !!app().sheet?.open,
              onClose: () => app().closeSheet(),
              agentId,
            }),
          ),
        ),
      );
    });
  };
  const accountRow = () =>
    [...host.querySelectorAll<HTMLButtonElement>(".row")].find(
      (r) => r.querySelector(".lbl")?.textContent === "Account",
    );

  beforeAll(() => {
    host = document.createElement("div");
    document.body.append(host);
    root = createRoot(host);
  });
  afterEach(() => act(() => app().closeSheet()));
  afterAll(() => {
    act(() => root.unmount());
    host.remove();
  });

  it("names the account and is read-only while a turn runs", async () => {
    await render("pamukkale");
    await vi.waitFor(() => expect(accountRow()).toBeDefined());
    const row = accountRow() as HTMLButtonElement;
    expect(row.querySelector(".val")?.textContent).toBe("work");
    expect(row.disabled).toBe(true);
    expect(row.textContent).toContain("Switch once the turn finishes");
  });

  it("points at the Mac when there is no other usable account", async () => {
    // Codex's second account was never signed in.
    await render("caspian");
    await vi.waitFor(() => expect(accountRow()).toBeDefined());
    const row = accountRow() as HTMLButtonElement;
    expect(row.querySelector(".val")?.textContent).toBe("Terminal login");
    expect(row.disabled).toBe(true);
    expect(row.textContent).toContain("Add accounts on the Mac");
  });

  it("opens the picker and moves the agent on a pick", async () => {
    await render("kamakura");
    await vi.waitFor(() => expect(accountRow()?.disabled).toBe(false));
    await act(async () => accountRow()?.click());
    expect(app().sheet).toMatchObject({ name: "accountPicker", props: { agentId: "kamakura" } });
    await render("kamakura");

    const options = () =>
      [...host.querySelectorAll<HTMLButtonElement>(".sheet.stacked .row")].map((r) => ({
        label: r.querySelector(".lbl")?.textContent,
        checked: !!r.querySelector(".check-ic"),
      }));
    await vi.waitFor(() =>
      expect(options()).toEqual([
        { label: "Terminal login", checked: false },
        { label: "work", checked: true },
      ]),
    );

    const terminal = [...host.querySelectorAll<HTMLButtonElement>(".sheet.stacked .row")][0];
    await act(async () => terminal.click());

    await vi.waitFor(() => expect(stampOf("kamakura")).toBeNull());
    expect(app().sheet?.open).toBe(false);
  });

  it("leaves another agent's picker alone when an earlier pick's answer lands", async () => {
    // The host answers slowly; the user dismisses the picker and opens
    // another agent's before it does.
    const rows = () => [...host.querySelectorAll<HTMLButtonElement>(".sheet.stacked .row")];
    const openPickerFor = async (agentId: string) => {
      await act(async () => app().openSheet("accountPicker", withHost(MOCK_HOST_KEY, { agentId })));
      await render(agentId);
      await vi.waitFor(() => expect(rows().length).toBe(2));
    };
    let reply = deferred<unknown>();
    const held = () => {
      reply = deferred<unknown>();
      return reply.promise;
    };

    // A success for kamakura must not close arabia's picker.
    // The row that is not the current account: the current one only closes.
    const other = () => rows().find((r) => !r.querySelector(".check-ic")) as HTMLButtonElement;
    const first = vi.spyOn(api, "switchAgentAccount").mockImplementationOnce(() => held() as never);
    await openPickerFor("kamakura");
    await act(async () => other().click());
    expect(first).toHaveBeenCalledTimes(1);
    await act(async () => app().closeSheet());
    await openPickerFor("arabia");
    await act(async () => {
      reply.resolve({ ...agentOf(hostOnScreen().getState(), "kamakura"), account: "work" });
    });
    expect(app().sheet).toMatchObject({
      name: "accountPicker",
      open: true,
      props: { agentId: "arabia" },
    });
    expect(stampOf("kamakura")).toBe("work");

    // A refusal for kamakura must not show in arabia's picker.
    await act(async () => app().closeSheet());
    await openPickerFor("kamakura");
    const second = vi
      .spyOn(api, "switchAgentAccount")
      .mockImplementationOnce(() => held() as never);
    await act(async () => other().click());
    expect(second).toHaveBeenCalledTimes(2);
    await act(async () => app().closeSheet());
    await openPickerFor("arabia");
    await act(async () => {
      reply.reject(new Error("Wait for the turn to finish before switching accounts."));
    });
    expect(host.querySelector(".notice-bar.error")).toBeNull();
    expect(app().sheet).toMatchObject({
      name: "accountPicker",
      open: true,
      props: { agentId: "arabia" },
    });
  });
});

describe("a switch against the workspace reads around it", () => {
  type Snapshot = Awaited<ReturnType<typeof api.getWorkspace>>;

  /** Hold the next workspace read at the host: it reads its answer at once
   *  (so the answer predates whatever follows), then waits to be released.
   *  `armed` resolves once it holds — the moment to raise what overtakes it. */
  const hold = (shape: (snapshot: Snapshot) => Snapshot = (s) => s) => {
    const read = api.getWorkspace;
    let release: () => void = () => {};
    let arm: () => void = () => {};
    const armed = new Promise<void>((resolve) => {
      arm = resolve;
    });
    vi.spyOn(api, "getWorkspace").mockImplementationOnce(async () => {
      const snapshot = await read();
      arm();
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return shape(snapshot);
    });
    return { armed, release: () => release() };
  };

  it("is not undone by an older workspace read that answers after it", async () => {
    // In step with the host first: the picker tests above answered a held
    // switch with a record of their own.
    await host().refreshWorkspace();
    const was = stampOf("kamakura") ?? null;
    const target = was ? "default" : "work";
    const held = hold();
    const stale = host().refreshWorkspace();
    await held.armed;
    await host().switchAgentAccount("kamakura", target);
    const switched = stampOf("kamakura") ?? null;
    expect(switched).not.toBe(was);
    held.release();
    await stale;
    expect(stampOf("kamakura") ?? null).toBe(switched);
  });

  it("lands only the newest of two workspace reads in flight", async () => {
    const held = hold();
    const first = host().refreshWorkspace();
    await held.armed;
    await host().refreshWorkspace();
    const after = host().workspace;
    held.release();
    await first;
    expect(host().workspace).toBe(after);
  });

  it("replays a branch change onto the read's own repo list", async () => {
    // The host added a repo the phone has not seen; a branch event for the
    // primary repo lands while the read that would show it is out.
    const held = hold(
      (snapshot) =>
        snapshot && {
          ...snapshot,
          agents: snapshot.agents.map((a) =>
            a.id === "kamakura"
              ? {
                  ...a,
                  repos: [...a.repos, { ...a.repos[0], repo_path: "/x/extra", subdir: "extra" }],
                }
              : a,
          ),
        },
    );
    const primary = agentOf(host(), "kamakura")?.repos[0].subdir ?? "";
    const pending = host().refreshWorkspace();
    await held.armed;
    hostEvent("agent:branch", { agent_id: "kamakura", subdir: primary, branch: "feat/overtook" });
    held.release();
    await pending;
    const repos = agentOf(host(), "kamakura")?.repos ?? [];
    expect(repos.map((r) => r.subdir)).toContain("extra");
    expect(repos.find((r) => r.subdir === primary)?.branch).toBe("feat/overtook");
  });

  it("merges a repo added meanwhile by identity", async () => {
    const held = hold();
    const base = agentOf(host(), "kamakura")?.repos[0];
    const added = { ...(base as NonNullable<typeof base>), repo_path: "/x/added", subdir: "added" };
    const pending = host().refreshWorkspace();
    await held.armed;
    hostEvent("agent:repo_added", { agent_id: "kamakura", repo: added });
    // Delivered twice, as a reconnect can: still one row.
    hostEvent("agent:repo_added", { agent_id: "kamakura", repo: added });
    held.release();
    await pending;
    const repos = agentOf(host(), "kamakura")?.repos ?? [];
    expect(repos.filter((r) => r.repo_path === "/x/added")).toHaveLength(1);
    // Back in step with the host, which never had it.
    await host().refreshWorkspace();
    expect(agentOf(host(), "kamakura")?.repos.some((r) => r.subdir === "added")).toBe(false);
  });
});
