// @vitest-environment happy-dom
// The run-target picker on the new-workspace row. With no paired host it must
// be the workspace-type pill exactly as it was, so the first test draws that
// pill's markup the way the pill always has and compares the two.

import { beforeEach, describe, expect, it, vi } from "vitest";

const { availableWorkspaceTypes, projectDefinition } = vi.hoisted(() => ({
  availableWorkspaceTypes: vi.fn(),
  projectDefinition: vi.fn(),
}));
vi.mock("@/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api")>();
  return { ...actual, api: { availableWorkspaceTypes, projectDefinition } };
});

import type { ProjectDefinition, WorkspaceTypeOption } from "@/api";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { useAppStore } from "@/store";
import { type EnvironmentEntry, LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { fireEvent, render, screen } from "@/test/render";
import { RunTargetPicker } from ".";

const OPTIONS: WorkspaceTypeOption[] = [
  { kind: "host", available: true, reason: null, default: true },
  { kind: "box", available: false, reason: "VM image not installed", default: false },
];

const DEFINITION: ProjectDefinition = {
  project_id: "p1",
  name: "App",
  home: { device: "this-mac", name: "MacBook" },
  repos: [{ id: "r1", dir: "app", origin: "https://github.com/acme/app.git", label: null }],
};

const studio = (over: Partial<EnvironmentEntry> = {}): EnvironmentEntry => ({
  id: "studio",
  name: "Studio",
  kind: "remote",
  connection: "connected",
  protocol: { version: 9, ops: ["ensure_project"], events: [], features: [] },
  ...over,
});

function picker(onPick = vi.fn()) {
  return render(
    <RunTargetPicker repoPath="/w/app" provider="claude" projectId="p1" onPick={onPick} />,
  );
}

/** The workspace-type pill as it has always been drawn for `OPTIONS`, closed
 *  and open. */
function pillAsBefore(open: boolean): string {
  const drawn = render(
    <span style={{ position: "relative" }}>
      <button
        type="button"
        className="pill is-action tip"
        data-tip="Runs in the macOS sandbox"
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <Icon name="folderLock" />
        <span style={{ color: "var(--fg-2)" }}>in</span>
        <span className="v">macOS sandbox</span>
        <Icon name="chevD" size={9} />
      </button>
      {open && (
        <>
          <div style={{ position: "fixed", inset: 0, zIndex: 199 }} />
          <DropdownMenu className="ws-type-menu" style={{ bottom: "calc(100% + 6px)", left: 0 }}>
            <DropdownSection>Runs in</DropdownSection>
            <DropdownItem
              as="button"
              role="menuitemradio"
              aria-checked={true}
              active={true}
              disabled={false}
            >
              <Icon name="folderLock" size={14} />
              <span className="di-l">macOS sandbox</span>
              <span className="di-m">default</span>
              <Icon name="check" size={13} />
            </DropdownItem>
            <DropdownItem
              as="button"
              role="menuitemradio"
              aria-checked={false}
              active={false}
              disabled={true}
              title="VM image not installed"
            >
              <Icon name="cube" size={14} />
              <span className="di-l">Linux VM</span>
            </DropdownItem>
          </DropdownMenu>
        </>
      )}
    </span>,
  );
  const html = drawn.container.innerHTML;
  drawn.unmount();
  return html;
}

beforeEach(() => {
  vi.clearAllMocks();
  availableWorkspaceTypes.mockResolvedValue(OPTIONS);
  projectDefinition.mockResolvedValue(DEFINITION);
  useAppStore.setState(useAppStore.getInitialState(), true);
});

describe("RunTargetPicker with no paired host", () => {
  it("is the workspace-type pill exactly as before, closed and open", async () => {
    const closed = pillAsBefore(false);
    const opened = pillAsBefore(true);
    const { container } = picker();
    const pill = await screen.findByRole("button", { name: /macOS sandbox/ });

    expect(container.innerHTML).toBe(closed);
    fireEvent.click(pill);
    expect(container.innerHTML).toBe(opened);
    expect(screen.queryByText("This Mac")).toBeNull();
    expect(projectDefinition).not.toHaveBeenCalled();
  });

  it("keeps a pick on This Mac", async () => {
    const onPick = vi.fn();
    picker(onPick);
    fireEvent.click(await screen.findByRole("button", { name: /macOS sandbox/ }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: /macOS sandbox/ }));
    expect(onPick).toHaveBeenCalledWith(LOCAL_ENVIRONMENT_ID, "host");
  });
});

describe("RunTargetPicker with a paired host", () => {
  it("says where the workspace runs and lists This Mac's types beside the host", async () => {
    useAppStore.getState().upsertEnvironment(studio());
    const onPick = vi.fn();
    picker(onPick);

    const pill = await screen.findByRole("button", { name: /on\s*This Mac\s*in\s*macOS sandbox/ });
    fireEvent.click(pill);

    expect(screen.getByText("Paired hosts")).toBeTruthy();
    const host = screen.getByRole("menuitemradio", { name: "Studio" }) as HTMLButtonElement;
    expect(host.disabled).toBe(false);
    expect(projectDefinition).toHaveBeenCalledWith("p1");

    fireEvent.click(host);
    expect(onPick).toHaveBeenCalledWith("studio", undefined);
  });

  it("shows an offline host disabled, its connection as the reason", async () => {
    useAppStore.getState().upsertEnvironment(studio({ connection: "disconnected" }));
    picker();
    fireEvent.click(await screen.findByRole("button", { name: /This Mac/ }));
    const host = screen.getByRole("menuitemradio", { name: "Studio" }) as HTMLButtonElement;
    expect(host.disabled).toBe(true);
    expect(host.title).toBe("Studio · offline");
  });

  it("says why the project can't go to a host once it is read", async () => {
    useAppStore.getState().upsertEnvironment(studio());
    projectDefinition.mockResolvedValue({
      ...DEFINITION,
      repos: [{ ...DEFINITION.repos[0], origin: null }],
    });
    picker();

    fireEvent.click(await screen.findByRole("button", { name: /This Mac/ }));
    await vi.waitFor(() => {
      const host = screen.getByRole("menuitemradio", { name: "Studio" }) as HTMLButtonElement;
      expect(host.disabled).toBe(true);
      expect(host.title).toBe("app has no remote. Publish it first.");
    });
  });
});
