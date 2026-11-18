// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import { useAppStore } from "@/store";
import { EngineScope } from "@/store/EngineScope";
import type { EnvironmentEntry } from "@/store/environments";
import { render, screen } from "@/test/render";
import { EnvironmentFrame } from ".";

const studio = (over: Partial<EnvironmentEntry> = {}): EnvironmentEntry => ({
  id: "studio",
  name: "Studio",
  kind: "remote",
  connection: "connected",
  ...over,
});

const frame = (container: HTMLElement) => container.querySelector(".env-frame") as HTMLElement;

beforeEach(() => useAppStore.setState(useAppStore.getInitialState(), true));

describe("EnvironmentFrame", () => {
  it("draws no tab in a window with no paired host", () => {
    const { container } = render(
      <EnvironmentFrame>
        <textarea />
      </EnvironmentFrame>,
    );
    expect(container.querySelector(".env-tab")).toBeNull();
    expect(frame(container).dataset.env).toBe("local");
  });

  it("names This Mac once a host is paired, without tinting it", () => {
    useAppStore.getState().upsertEnvironment(studio());
    const { container } = render(
      <EnvironmentFrame>
        <textarea />
      </EnvironmentFrame>,
    );
    expect(container.querySelector(".env-tab")?.textContent).toBe("This Mac");
    expect(container.querySelector("button.env-tab")).toBeNull();
    expect(frame(container).dataset.env).toBe("local");
    // A laptop like any device's, with no connection dot: it is always here.
    expect(container.querySelector(".env-tab .device-glyph svg")).not.toBeNull();
    expect(container.querySelector(".env-tab .env-dot")).toBeNull();
  });

  it("is a paired host's own while scoped to it, and greyed while it is out of reach", () => {
    useAppStore.getState().upsertEnvironment(studio());
    const scoped = (
      <EngineScope hostId="studio">
        <EnvironmentFrame>
          <textarea />
        </EnvironmentFrame>
      </EngineScope>
    );
    const { container, rerender } = render(scoped);
    expect(container.querySelector(".env-tab")?.textContent).toBe("Studio");
    expect(frame(container).dataset.env).toBe("remote");
    expect(frame(container).dataset.tone).toBe("connected");
    // The same laptop, with the host's connection on its corner.
    const dot = container.querySelector(".env-tab .device-glyph .env-dot") as HTMLElement;
    expect(container.querySelector(".env-tab .device-glyph svg")).not.toBeNull();
    expect(dot.dataset.state).toBe("connected");

    useAppStore.getState().setEnvironmentConnection("studio", "disconnected");
    rerender(scoped);
    expect(frame(container).dataset.tone).toBe("inactive");
  });

  // The tint is keyed on `connected` alone, so every other tone has to be told
  // apart from it here: a host stopped on an error nobody is retrying is out
  // of reach too, and one still dialling is not in reach yet.
  it("is not a connected host's while it has stopped on an error or is still dialling", () => {
    useAppStore.getState().upsertEnvironment(studio());
    const scoped = (
      <EngineScope hostId="studio">
        <EnvironmentFrame>
          <textarea />
        </EnvironmentFrame>
      </EngineScope>
    );
    const { container, rerender } = render(scoped);

    useAppStore.getState().setEnvironmentConnection("studio", "error", "unpaired", false);
    rerender(scoped);
    expect(frame(container).dataset.tone).toBe("problem");

    useAppStore.getState().setEnvironmentConnection("studio", "connecting");
    rerender(scoped);
    expect(frame(container).dataset.tone).toBe("connecting");
  });

  it("draws the tab it is given in place of its own", () => {
    useAppStore.getState().upsertEnvironment(studio());
    render(
      <EnvironmentFrame tab={<button type="button">pick a place</button>}>
        <textarea />
      </EnvironmentFrame>,
    );
    expect(screen.getByRole("button", { name: "pick a place" })).toBeTruthy();
    expect(screen.queryByText("This Mac")).toBeNull();
  });
});
