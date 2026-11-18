// @vitest-environment happy-dom
// Settings › Cloud: a provider is connected with a token the provider has
// checked, and disconnected in one click. The token is in the form until it is
// kept and nowhere after; a refusal is shown beside the form with what was
// typed still there; a provider this pane does not know yet still gets its row.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type CloudProviderId, type CloudProviderStatus } from "@/api";
import { useAppStore } from "@/store";
import { fireEvent, render, screen } from "@/test/render";
import { CloudPane } from ".";

const fly = (over: Partial<CloudProviderStatus> = {}): CloudProviderStatus => ({
  id: "fly",
  label: "Fly.io",
  connected: false,
  account: null,
  ...over,
});

/** A promise the test settles by hand, to hold a connect in flight. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const mount = async (providers: CloudProviderStatus[]) => {
  vi.spyOn(api, "cloudProviders").mockResolvedValue(providers);
  render(<CloudPane />);
  await screen.findByText(providers[0].label);
};

const tokenField = () => screen.getByLabelText("Token") as HTMLInputElement;
const orgField = () => screen.getByLabelText("Organization") as HTMLInputElement;
const connectButton = () => screen.getByRole("button", { name: /Connect Fly\.io|Connecting/ });

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState(), true);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("Settings › Cloud", () => {
  it("offers Fly.io a token, an organization defaulting to personal, and Connect", async () => {
    await mount([fly()]);
    expect(screen.getByText("Not connected")).toBeTruthy();
    expect(tokenField().type).toBe("password");
    expect(orgField().placeholder).toBe("personal");
    expect(screen.getByText(/fly tokens create org/)).toBeTruthy();
    expect(screen.getByText(/kept in this Mac's keychain/)).toBeTruthy();
    // Nothing to send until there is a token.
    expect(connectButton()).toHaveProperty("disabled", true);
    fireEvent.change(tokenField(), { target: { value: "FlyV1 secret" } });
    expect(connectButton()).toHaveProperty("disabled", false);
  });

  it("connects, shows the organization, and keeps no token in the form", async () => {
    const connect = vi
      .spyOn(api, "cloudConnect")
      .mockResolvedValue(fly({ connected: true, account: "acme" }));
    vi.spyOn(api, "cloudDisconnect").mockResolvedValue(undefined);
    await mount([fly()]);

    fireEvent.change(tokenField(), { target: { value: " FlyV1 secret " } });
    fireEvent.change(orgField(), { target: { value: "acme" } });
    fireEvent.click(connectButton());

    expect(await screen.findByText("Connected to the acme organization")).toBeTruthy();
    expect(connect).toHaveBeenCalledWith("fly", "FlyV1 secret", "acme");
    expect(screen.queryByLabelText("Token")).toBeNull();

    // Back to the form: it starts empty, so the token was held nowhere else.
    fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(await screen.findByLabelText("Token")).toHaveProperty("value", "");
    expect(orgField().value).toBe("");
  });

  it("sends an empty organization for the provider's default", async () => {
    const connect = vi
      .spyOn(api, "cloudConnect")
      .mockResolvedValue(fly({ connected: true, account: "personal" }));
    await mount([fly()]);

    fireEvent.change(tokenField(), { target: { value: "FlyV1 secret" } });
    fireEvent.keyDown(tokenField(), { key: "Enter" });

    expect(await screen.findByText("Connected to the personal organization")).toBeTruthy();
    expect(connect).toHaveBeenCalledWith("fly", "FlyV1 secret", "");
  });

  it("holds Connect while the provider checks the token", async () => {
    const answer = deferred<CloudProviderStatus>();
    vi.spyOn(api, "cloudConnect").mockReturnValue(answer.promise);
    await mount([fly()]);

    fireEvent.change(tokenField(), { target: { value: "FlyV1 secret" } });
    fireEvent.click(connectButton());

    expect(connectButton().textContent).toBe("Connecting…");
    expect(connectButton()).toHaveProperty("disabled", true);
    expect(tokenField()).toHaveProperty("disabled", true);

    answer.resolve(fly({ connected: true, account: "acme" }));
    expect(await screen.findByText("Connected to the acme organization")).toBeTruthy();
  });

  it("shows the provider's refusal beside the form, with what was typed still there", async () => {
    vi.spyOn(api, "cloudConnect").mockRejectedValue("Fly.io refused this token");
    await mount([fly()]);

    fireEvent.change(tokenField(), { target: { value: "FlyV1 wrong" } });
    fireEvent.change(orgField(), { target: { value: "acme" } });
    fireEvent.click(connectButton());

    expect(await screen.findByText("Fly.io refused this token")).toBeTruthy();
    expect(screen.getByText("Not connected")).toBeTruthy();
    expect(tokenField().value).toBe("FlyV1 wrong");
    expect(orgField().value).toBe("acme");
    expect(connectButton()).toHaveProperty("disabled", false);

    // Editing the token clears the refusal.
    fireEvent.change(tokenField(), { target: { value: "FlyV1 right" } });
    expect(screen.queryByText("Fly.io refused this token")).toBeNull();
  });

  it("disconnects back to the form, and says the machines stay", async () => {
    const disconnect = vi.spyOn(api, "cloudDisconnect").mockResolvedValue(undefined);
    await mount([fly({ connected: true, account: "acme" })]);
    expect(screen.queryByText(/not deleted by disconnecting/)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));

    expect(await screen.findByLabelText("Token")).toBeTruthy();
    expect(disconnect).toHaveBeenCalledWith("fly");
    expect(
      screen.getByText("Machines already created at Fly.io are not deleted by disconnecting."),
    ).toBeTruthy();
  });

  it("keeps a provider connected when the disconnect fails, and says why", async () => {
    vi.spyOn(api, "cloudDisconnect").mockRejectedValue("keychain locked");
    await mount([fly({ connected: true, account: "acme" })]);

    fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));

    expect(await screen.findByText("keychain locked")).toBeTruthy();
    expect(screen.getByText("Connected to the acme organization")).toBeTruthy();
  });

  it("gives a provider it does not know yet its row, with no form", async () => {
    await mount([
      { id: "daytona" as CloudProviderId, label: "Daytona", connected: false, account: null },
      { id: "e2b" as CloudProviderId, label: "E2B", connected: true, account: "team" },
      fly(),
    ]);

    expect(screen.getByText("Daytona")).toBeTruthy();
    expect(screen.getByText("E2B")).toBeTruthy();
    expect(screen.getByText("Connected as team")).toBeTruthy();
    expect(screen.getAllByText("Not connected")).toHaveLength(2);
    // Only Fly.io has a form; E2B can still be disconnected.
    expect(screen.getAllByLabelText("Token")).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "Disconnect" })).toHaveLength(1);
  });

  it("says when the providers could not be read", async () => {
    vi.spyOn(api, "cloudProviders").mockRejectedValue("keychain locked");
    render(<CloudPane />);
    expect(
      await screen.findByText("Couldn't read the cloud providers: keychain locked"),
    ).toBeTruthy();
    expect(screen.queryByLabelText("Token")).toBeNull();
  });
});
