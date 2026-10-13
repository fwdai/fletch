// Moving one agent onto another account, end to end over the mock host: the
// real protocol client, the real store slice, the real More sheet and picker.
// The jsdom URL in vite.config.ts puts this file in mock mode (see
// `mockEnabled`). The mock refuses the way the engine does
// (`supervisor/account_switch.rs`), so the refusals here are the host's words.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { AccountPickerSheet } from "../src/sheets/AccountPickerSheet";
import { AgentMoreSheet } from "../src/sheets/AgentMoreSheet";
import { agentOf, api, useStore } from "../src/store";

const state = () => useStore.getState();
const stampOf = (id: string) => agentOf(state(), id)?.account;

beforeAll(async () => {
  await state().init();
  await vi.waitFor(() => expect(state().connection).toBe("connected"), { timeout: 5000 });
  await state().loadProviderAccounts();
});

describe("switchAgentAccount over the mock host", () => {
  it("restamps the agent with the account the host hands back", async () => {
    expect(stampOf("kamakura")).toBe("work");

    const record = await state().switchAgentAccount("kamakura", "default");

    // The host spells the CLI's own login as no stamp.
    expect(record?.account).toBeNull();
    expect(stampOf("kamakura")).toBeNull();
    expect(state().switchingAccount.kamakura).toBe(false);
    // Only this agent moved.
    expect(stampOf("arabia")).toBe("work");
  });

  it("refuses the account the agent already runs under", async () => {
    await expect(state().switchAgentAccount("kamakura", "default")).rejects.toThrow(
      "This agent already runs under the `default` account.",
    );
  });

  it("refuses a signed-out target and leaves the stamp alone", async () => {
    await expect(state().switchAgentAccount("caspian", "personal")).rejects.toThrow(
      "The `personal` account isn't signed in",
    );
    expect(stampOf("caspian")).toBeNull();
    expect(state().switchingAccount.caspian).toBe(false);
  });

  it("refuses mid-turn", async () => {
    // Held on a tool approval for the whole run, so it stays mid-turn.
    expect(agentOf(state(), "pamukkale")?.status).toBe("running");
    await expect(state().switchAgentAccount("pamukkale", "default")).rejects.toThrow(
      "Wait for the turn to finish before switching accounts.",
    );
    expect(stampOf("pamukkale")).toBe("work");
  });

  it("restamps a planning chat in the chat registry", async () => {
    await state().loadChats("prj-fletch");
    expect(stampOf("sakura")).toBe("work");

    await state().switchAgentAccount("sakura", "default");

    expect(state().chats["prj-fletch"].find((c) => c.id === "sakura")?.account).toBeNull();
  });

  it("holds only until the provider's selection next moves every agent", async () => {
    await state().setActiveProviderAccount("claude", "default");
    await state().setActiveProviderAccount("claude", "work");

    await vi.waitFor(() => expect(stampOf("kamakura")).toBe("work"));
  });
});

describe("the More sheet's account row and its picker", () => {
  let host: HTMLDivElement;
  let root: Root;

  const render = async (agentId: string) => {
    await act(async () => {
      root.render(
        createElement(
          "div",
          null,
          createElement(AgentMoreSheet, { open: true, onClose: () => {}, agentId }),
          createElement(AccountPickerSheet, {
            open: state().sheet?.name === "accountPicker" && !!state().sheet?.open,
            onClose: () => state().closeSheet(),
            agentId,
          }),
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
  afterEach(() => act(() => state().closeSheet()));
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
    expect(state().sheet).toMatchObject({ name: "accountPicker", props: { agentId: "kamakura" } });
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
    expect(state().sheet?.open).toBe(false);
  });

  it("leaves another agent's picker alone when an earlier pick's answer lands", async () => {
    // The host answers slowly; the user dismisses the picker and opens
    // another agent's before it does.
    const rows = () => [...host.querySelectorAll<HTMLButtonElement>(".sheet.stacked .row")];
    const openPickerFor = async (agentId: string) => {
      await act(async () => state().openSheet("accountPicker", { agentId }));
      await render(agentId);
      await vi.waitFor(() => expect(rows().length).toBe(2));
    };
    let settle: { resolve: (r: unknown) => void; reject: (e: Error) => void } = {
      resolve: () => {},
      reject: () => {},
    };
    const held = () =>
      new Promise((resolve, reject) => {
        settle = { resolve, reject };
      });

    // A success for kamakura must not close arabia's picker.
    vi.spyOn(api, "switchAgentAccount").mockImplementationOnce(() => held() as never);
    await openPickerFor("kamakura");
    await act(async () => rows()[1].click());
    await act(async () => state().closeSheet());
    await openPickerFor("arabia");
    await act(async () => {
      settle.resolve({ ...agentOf(state(), "kamakura"), account: "work" });
    });
    expect(state().sheet).toMatchObject({
      name: "accountPicker",
      open: true,
      props: { agentId: "arabia" },
    });
    expect(stampOf("kamakura")).toBe("work");

    // A refusal for arabia must not show in kamakura's picker — but the
    // picker it was asked from does show it.
    vi.spyOn(api, "switchAgentAccount").mockImplementationOnce(() => held() as never);
    await act(async () => rows()[0].click());
    await act(async () => state().closeSheet());
    await openPickerFor("kamakura");
    await act(async () => {
      settle.reject(new Error("Wait for the turn to finish before switching accounts."));
    });
    expect(host.querySelector(".notice-bar.error")).toBeNull();
    expect(state().sheet).toMatchObject({
      name: "accountPicker",
      open: true,
      props: { agentId: "kamakura" },
    });
  });
});
