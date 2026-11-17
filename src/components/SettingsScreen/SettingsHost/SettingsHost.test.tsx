// @vitest-environment happy-dom
// Settings' own host choice. With no host paired there is nothing to choose
// and nothing on screen says otherwise. With one, a pane of host-owned settings
// reads the picked host's values and writes through its engine, a pane about
// this desktop stays This Mac's, and the rest of the window keeps showing what
// it showed.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/** Every op this Mac's engine is asked, by name: the store's own `api` is
 *  whatever `@/api` exports, so a proxy there records them all. */
const { localCalls } = vi.hoisted(() => ({ localCalls: [] as string[] }));
vi.mock("@/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api")>();
  const api = new Proxy(
    {},
    {
      get: (_target, op) => () => {
        localCalls.push(String(op));
        return Promise.resolve(undefined);
      },
    },
  );
  return { ...actual, api };
});

// Providers draws each brand icon from a fetched SVG, which has nothing to say here.
vi.mock("@/components/ProviderIcon", () => ({ ProviderIcon: () => null }));

import type { Transport } from "@/api/transport";
import type { SettingsSection } from "@/storage/preferences";
import { hostStore, stopHost, useAppStore } from "@/store";
import { requiredOps } from "@/store/capabilities";
import { act, fireEvent, render, screen } from "@/test/render";
import { SettingsScreen } from "..";

const STUDIO = "studio";

/** What the studio's engine was asked, op and arguments. */
let studioCalls: { op: string; args?: Record<string, unknown> }[] = [];

const transport: Transport = {
  call: <T,>(op: string, args?: Record<string, unknown>) => {
    studioCalls.push({ op, args });
    return Promise.resolve(undefined as T);
  },
  on: () => Promise.resolve(() => {}),
};

/** Pair the studio the way the registry publishes a host: an entry with its
 *  connection, and a partition with it. It answers the host settings ops but
 *  not `review_start`, so its review gate is closed. */
function pairStudio(connection: "connected" | "disconnected" = "connected") {
  useAppStore.getState().upsertEnvironment({
    id: STUDIO,
    name: "Studio",
    kind: "remote",
    connection,
    transport,
    protocol: { version: 2, ops: [...requiredOps("hostSettings")], events: [], features: [] },
  });
}

const open = (section: SettingsSection) => {
  useAppStore.setState({ settingsSection: section });
  return render(<SettingsScreen />);
};

const goTo = (section: SettingsSection) =>
  act(() => useAppStore.getState().setSettingsSection(section));

/** The switch on the row titled `title`. */
const switchFor = (title: string) => {
  const row = screen.getByText(title).closest(".set-row");
  const toggle = row?.querySelector<HTMLButtonElement>("[role='switch']");
  if (!toggle) throw new Error(`no switch on "${title}"`);
  return toggle;
};

const picker = () => screen.queryByRole("button", { name: "Settings for" });

const pick = (name: string) => {
  const trigger = picker();
  if (!trigger) throw new Error("no host picker");
  fireEvent.click(trigger);
  const option = screen.getAllByRole("option").find((o) => o.textContent?.includes(name));
  if (!option) throw new Error(`no option for ${name}`);
  fireEvent.click(option);
};

beforeEach(() => {
  localCalls.length = 0;
  studioCalls = [];
  useAppStore.setState(useAppStore.getInitialState(), true);
});

afterEach(async () => {
  await stopHost(STUDIO);
  useAppStore.getState().removeEnvironment(STUDIO);
});

describe("Settings with no host paired", () => {
  it.each<SettingsSection>(["general", "git", "sandbox", "network", "providers", "tools"])(
    "shows no host picker on %s",
    (section) => {
      const { container } = open(section);
      expect(picker()).toBeNull();
      expect(screen.queryByText("Settings for")).toBeNull();
      expect(container.querySelector(".set-host-pick")).toBeNull();
    },
  );
});

describe("Settings with a paired host", () => {
  beforeEach(() => {
    pairStudio();
    // Two different answers, so the row shows whose it is reading.
    useAppStore.setState({ notifyTurnComplete: true });
    hostStore(STUDIO).setState({ notifyTurnComplete: false });
  });

  it("starts on This Mac, and lists the host with its connection", () => {
    open("general");

    expect(picker()?.textContent).toContain("This Mac");
    expect(switchFor("Turn finished").getAttribute("aria-checked")).toBe("true");

    fireEvent.click(picker() as HTMLElement);
    const options = screen.getAllByRole("option").map((o) => o.textContent);
    expect(options).toEqual(["This Mac", expect.stringContaining("Studio")]);
    expect(options[1]).toContain("connected");
  });

  it("reads and writes the picked host's setting through its engine", () => {
    open("general");

    pick("Studio");
    expect(switchFor("Turn finished").getAttribute("aria-checked")).toBe("false");

    fireEvent.click(switchFor("Turn finished"));

    expect(studioCalls).toContainEqual({
      op: "set_notify_turn_complete",
      args: { enabled: true },
    });
    expect(localCalls).not.toContain("setNotifyTurnComplete");
    expect(useAppStore.getState().hostStates[STUDIO].notifyTurnComplete).toBe(true);
    expect(useAppStore.getState().notifyTurnComplete).toBe(true);
  });

  it("leaves the host on screen alone", () => {
    open("general");

    pick("Studio");

    expect(useAppStore.getState().activeEnvironmentId).toBe("local");
  });

  it("keeps a pane about this desktop on This Mac, and the pick for the next host pane", () => {
    open("general");
    pick("Studio");

    // Review's switches act on this Mac's settings: no picker, and the studio's
    // closed review gate does not reach them.
    goTo("review");
    expect(picker()).toBeNull();
    expect(switchFor("Review on PR open").disabled).toBe(false);

    goTo("git");
    expect(picker()?.textContent).toContain("Studio");
  });

  it("starts on This Mac again the next time Settings opens", () => {
    const first = open("general");
    pick("Studio");
    first.unmount();

    open("general");

    expect(picker()?.textContent).toContain("This Mac");
    expect(switchFor("Turn finished").getAttribute("aria-checked")).toBe("true");
  });

  it("offers an offline host, showing what was last known", () => {
    useAppStore.getState().setEnvironmentConnection(STUDIO, "disconnected", undefined, true);
    open("general");

    fireEvent.click(picker() as HTMLElement);
    const studio = screen.getAllByRole("option").find((o) => o.textContent?.includes("Studio"));
    expect(studio?.textContent).toContain("reconnecting");

    fireEvent.click(studio as HTMLElement);
    expect(switchFor("Turn finished").getAttribute("aria-checked")).toBe("false");
  });
});
