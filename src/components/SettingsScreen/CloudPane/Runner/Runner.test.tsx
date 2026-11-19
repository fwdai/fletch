// @vitest-environment happy-dom
// Settings › Cloud › the runner, under the provider it is at. With none, a
// form says what one is and costs and takes the Claude token; creating shows
// each step and, at the GitHub one, its code and where to enter it. A runner
// that exists shows its state in words and what it allows: Start, Stop,
// another device's code while it runs, and Delete, whose refusal offers the
// forced delete; its provider cannot be disconnected under it. Status is read
// again only while it is changing. (The runner's paired host follows its
// status: src/remote/hosts.test.ts.)

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listeners = vi.hoisted(() => new Map<string, (e: { payload: unknown }) => void>());
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (event: string, handler: (e: { payload: unknown }) => void) => {
    listeners.set(event, handler);
    return () => listeners.delete(event);
  }),
}));
const shell = vi.hoisted(() => ({ open: vi.fn(async () => {}) }));
vi.mock("@tauri-apps/plugin-shell", () => shell);
vi.mock("@/remote/device", () => ({
  thisDevice: async () => ({ name: "Studio", platform: "macos", appVersion: "0.8.29" }),
}));

import {
  api,
  type CloudProviderStatus,
  type CloudRunnerProgress,
  type CloudRunnerRecord,
  type CloudRunnerStatus,
} from "@/api";
import { useAppStore } from "@/store";
import { act, fireEvent, render, screen } from "@/test/render";
import { CloudPane } from "../index";

const fly = (over: Partial<CloudProviderStatus> = {}): CloudProviderStatus => ({
  id: "fly",
  label: "Fly.io",
  connected: true,
  account: "acme",
  account_label: "Organization",
  account_placeholder: "personal",
  token_placeholder: "FlyV1 fm2_…",
  token_help: "Run fly tokens create org in a terminal.",
  billing: "Fly.io bills its disk while the workspace exists.",
  ...over,
});

const RECORD: CloudRunnerRecord = {
  hostId: "runner-key",
  name: "Studio runner",
  relay: "wss://relay.example",
};
const NONE: CloudRunnerStatus = { state: "none", provider: null, record: null };
const at = (state: Exclude<CloudRunnerStatus["state"], "failed">): CloudRunnerStatus => ({
  state,
  provider: "fly",
  record: RECORD,
});
const GITHUB: CloudRunnerProgress = {
  step: "github_login",
  userCode: "ABCD-1234",
  verificationUrl: "https://github.com/login/device",
  expiresIn: 900,
};

/** A promise the test settles by hand, to hold a create in flight. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** What the engine reports as a create moves on. */
const emit = (step: CloudRunnerProgress) =>
  act(() => listeners.get("cloud-runner:progress")?.({ payload: step }));

/** The pane, with `runner` as every status read answers until a test says
 *  otherwise. Answers the status read's spy. */
const mount = (runner: CloudRunnerStatus, providers = [fly()]) => {
  vi.spyOn(api, "cloudProviders").mockResolvedValue(providers);
  const read = vi.spyOn(api, "cloudRunnerStatus").mockResolvedValue(runner);
  render(<CloudPane />);
  return read;
};

const tokenField = () => screen.getByLabelText("Claude token") as HTMLInputElement;

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState(), true);
  listeners.clear();
  vi.spyOn(api, "remoteDevicePublicKey").mockResolvedValue("device-key");
  vi.spyOn(api, "cloudSpares").mockResolvedValue(null);
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

describe("Settings › Cloud › runner", () => {
  it("with none, says what one is and costs, and takes the Claude token", async () => {
    mount(NONE);
    expect(await screen.findByText("No runner")).toBeTruthy();
    expect(screen.getByText(/agents keep running with this Mac closed/)).toBeTruthy();
    expect(
      screen.getByText(
        /Fly\.io bills your account for its machine while it runs and its disk until you delete it, and for the machines it makes there: one per workspace, and a spare it keeps ready\./,
      ),
    ).toBeTruthy();
    expect(tokenField().type).toBe("password");
    expect(screen.getByText("claude setup-token")).toBeTruthy();
    expect(screen.getByText(/paste what it prints/)).toBeTruthy();
    const create = screen.getByRole("button", { name: "Create runner" });
    expect(create).toHaveProperty("disabled", true);
    fireEvent.change(tokenField(), { target: { value: "sk-ant-oat01-secret" } });
    expect(create).toHaveProperty("disabled", false);
  });

  it("creates it, step by step, with the GitHub code to enter, then shows it", async () => {
    const answer = deferred<CloudRunnerRecord>();
    const create = vi.spyOn(api, "cloudRunnerCreate").mockReturnValue(answer.promise);
    const read = mount(NONE);
    await screen.findByText("No runner");

    fireEvent.change(tokenField(), { target: { value: " sk-ant-oat01-secret " } });
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Studio runner" } });
    fireEvent.click(screen.getByRole("button", { name: "Create runner" }));

    expect(await screen.findByText("Creating the runner…")).toBeTruthy();
    expect(screen.queryByLabelText("Claude token")).toBeNull();
    await vi.waitFor(() =>
      expect(create).toHaveBeenCalledWith("fly", {
        devicePublicKey: "device-key",
        deviceName: "Studio",
        devicePlatform: "macos",
        claudeToken: "sk-ant-oat01-secret",
        name: "Studio runner",
      }),
    );

    emit({ step: "creating_machine" });
    emit({ step: "installing_cli", cli: "Claude Code", version: "2.1.0" });
    expect(screen.getByText("Creating its machine and disk at Fly.io")).toBeTruthy();
    expect(screen.getByText("Installing Claude Code 2.1.0")).toBeTruthy();

    emit(GITHUB);
    expect(screen.getByText("Signing it in to GitHub")).toBeTruthy();
    expect(screen.getByText("ABCD-1234")).toBeTruthy();
    expect(
      screen.getByText("Waiting for the code to be entered. It lapses in 15 minutes."),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open GitHub" }));
    expect(shell.open).toHaveBeenCalledWith("https://github.com/login/device");

    read.mockResolvedValue(at("running"));
    await act(async () => answer.resolve(RECORD));
    expect(await screen.findByText("Studio runner · running")).toBeTruthy();
    expect(screen.queryByText("ABCD-1234")).toBeNull();
  });

  it("goes back to the form with the engine's words when a create fails", async () => {
    vi.spyOn(api, "cloudRunnerCreate").mockRejectedValue(
      "the GitHub code lapsed before it was entered",
    );
    mount(NONE);
    await screen.findByText("No runner");
    fireEvent.change(tokenField(), { target: { value: "sk-ant-oat01-secret" } });
    fireEvent.click(screen.getByRole("button", { name: "Create runner" }));

    expect(await screen.findByText("the GitHub code lapsed before it was entered")).toBeTruthy();
    expect(screen.getByText("No runner")).toBeTruthy();
    // The token went with the form: it is kept nowhere for a retry.
    expect(tokenField().value).toBe("");
  });

  it("running: its name and state, Stop, and a code for another device", async () => {
    vi.spyOn(api, "cloudRunnerPairingLink").mockResolvedValue({
      url: "fletch://pair?host=runner-key&addr=172.19.0.2:47285&relay=wss%3A%2F%2Frelay.example&token=K7PQ2M9X&name=Studio%20runner",
      code: "K7PQ2M9X",
      expiresAt: new Date(Date.now() + 300_000).toISOString(),
      preset: "full",
    });
    mount(at("running"));
    expect(await screen.findByText("Studio runner · running")).toBeTruthy();
    expect(screen.getByText(/keep running with this Mac closed/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Stop" }).getAttribute("data-tip")).toBe(
      "Stops its agents and their machines, then the runner's own. No device can reach it until it is started.",
    );
    expect(screen.queryByRole("button", { name: "Start" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Pair another device" }));
    expect(
      await screen.findByText(/code K7PQ2M9X · single use · lapses in (5:00|4:59)/),
    ).toBeTruthy();
    expect(screen.getByText("Scan the code with its camera.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Copy the pairing link" })).toBeTruthy();
  });

  it("stopped: Start, no Stop and no pairing, and the state read again after", async () => {
    const start = vi.spyOn(api, "cloudRunnerStart").mockResolvedValue(undefined);
    const read = mount(at("stopped"));
    expect(await screen.findByText("Studio runner · stopped")).toBeTruthy();
    expect(
      screen.getByText(
        "Its agents and their machines are stopped, and no device can reach it until you start it. Fly.io still bills for its disk, and for its workspaces' and spare machines while they exist.",
      ),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Pair another device" })).toBeNull();

    read.mockResolvedValue(at("running"));
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    expect(await screen.findByText("Studio runner · running")).toBeTruthy();
    expect(start).toHaveBeenCalledTimes(1);
  });

  it("its machine lost: a start makes a new one on the same disk", async () => {
    mount(at("machine_lost"));
    expect(await screen.findByText("Studio runner · machine lost")).toBeTruthy();
    expect(
      screen.getByText(/Start makes a new machine on the disk, and it is the same runner/),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Start" })).toBeTruthy();
  });

  it("its disk lost: cannot be recovered or started, only deleted", async () => {
    mount(at("disk_lost"));
    expect(await screen.findByText("Studio runner · disk lost")).toBeTruthy();
    expect(screen.getByText(/It can't be recovered: delete it and create a new one/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Start" })).toBeNull();
    expect(screen.getByRole("button", { name: "Delete runner" })).toBeTruthy();
  });

  it("failed: says why, in the provider's words", async () => {
    mount({ state: "failed", reason: "no capacity in ord", provider: "fly", record: RECORD });
    expect(await screen.findByText("Studio runner · failed")).toBeTruthy();
    expect(screen.getByText("Fly.io can't run it: no capacity in ord")).toBeTruthy();
  });

  it("an unfinished set-up: says so and offers Delete", async () => {
    mount({ state: "incomplete", provider: "fly", record: null });
    expect(await screen.findByText("Runner · set-up did not finish")).toBeTruthy();
    expect(screen.getByText(/Fletch quit while creating it/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Delete runner" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Start" })).toBeNull();
  });

  it("a delete refused shows why and offers the forced one, which says what may remain", async () => {
    const remove = vi
      .spyOn(api, "cloudRunnerDelete")
      .mockRejectedValueOnce(
        "Fletch can't reach the runner to have it destroy the machines it made: its disk is gone.",
      )
      .mockResolvedValueOnce({
        destroyed: [],
        warning:
          "The runner could not be reached, so the machines it made may remain at Fly.io. Delete them in its dashboard.",
      });
    const read = mount(at("disk_lost"));
    await screen.findByText("Studio runner · disk lost");

    fireEvent.click(screen.getByRole("button", { name: "Delete runner" }));
    expect(
      screen.getByText(
        "Delete the runner? The machines it made at Fly.io, its workspaces' and its spares, are deleted first, then its own machine and disk, and with them its history, logins and pairings.",
      ),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    expect(await screen.findByText(/Fletch can't reach the runner/)).toBeTruthy();
    expect(remove).toHaveBeenLastCalledWith(false);
    expect(
      screen.getByText(
        "Delete it anyway? Its own machine and disk are deleted, but the machines it made, its workspaces' and its spares, may remain at Fly.io, for you to delete there.",
      ),
    ).toBeTruthy();

    read.mockResolvedValue(NONE);
    fireEvent.click(screen.getByRole("button", { name: "Delete anyway" }));
    expect(
      await screen.findByText(/may remain at Fly\.io\. Delete them in its dashboard/),
    ).toBeTruthy();
    expect(remove).toHaveBeenLastCalledWith(true);
    expect(screen.getByText("No runner")).toBeTruthy();
  });

  it("its provider is not disconnected under it: the refusal shows on the connected row", async () => {
    const refusal =
      "Fly.io can't be disconnected while this machine's cloud runner is at Fly.io: the runner is billed while it exists and can only be managed through this connection. Delete the runner first.";
    const disconnect = vi.spyOn(api, "cloudDisconnect").mockRejectedValue(refusal);
    mount(at("running"));
    await screen.findByText("Studio runner · running");

    fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));

    expect(await screen.findByText(refusal)).toBeTruthy();
    expect(disconnect).toHaveBeenCalledWith("fly");
    expect(screen.getByText("Connected to the acme organization")).toBeTruthy();
    expect(screen.getByText("Studio runner · running")).toBeTruthy();
    expect(screen.queryByText(/Disconnecting deleted/)).toBeNull();
  });

  it("shows only under the provider its machine is at", async () => {
    mount(at("running"), [fly(), fly({ id: "daytona", label: "Daytona" })]);
    expect(await screen.findByText("Studio runner · running")).toBeTruthy();
    expect(screen.queryByText("No runner")).toBeNull();
  });

  it("reads its status again only while it is changing", async () => {
    vi.useFakeTimers();
    const read = mount(at("starting"));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(read).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(3_000));
    expect(read).toHaveBeenCalledTimes(2);

    read.mockResolvedValue(at("running"));
    await act(() => vi.advanceTimersByTimeAsync(3_000));
    expect(read).toHaveBeenCalledTimes(3);
    await act(() => vi.advanceTimersByTimeAsync(9_000));
    expect(read).toHaveBeenCalledTimes(3);
  });
});
