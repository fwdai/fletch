import { Fragment, lazy, Suspense, useMemo } from "react";
import { Icon, type IconName } from "@/components/Icon";
import type { SettingsSection } from "@/storage/preferences";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { IS_MAC } from "@/util/platform";
import { WorkflowsPane } from "@/workflows/builder";
import pkg from "../../../package.json";
import { AccountPane } from "./AccountPane";
import { CustomAgentsPane } from "./CustomAgents";
import { DictationPane } from "./Dictation";
import { ExperimentalPane } from "./ExperimentalPane";
import { GeneralPane } from "./GeneralPane";
import { GitPane } from "./GitPane";
import { LayoutPane } from "./LayoutPane";
import { McpServersPane } from "./McpServers";
import { ProvidersPane } from "./ProvidersPane";
import { RemoteControlPane } from "./RemoteControl";
import { SandboxPane } from "./Sandbox";
import { SkillsPane } from "./Skills";

// Lazily loaded — code-split into its own chunk, fetched only when the Developer
// section is actually opened. Visibility is gated at render (dev builds, or an
// admin user in production), not by dropping the chunk from the build.
const DeveloperPane = lazy(() =>
  import("./DeveloperPane").then((m) => ({ default: m.DeveloperPane })),
);

// Built-in sections carry explicit order weights so extension panes can slot
// *between* them via their own `order`, not just append. Each `group` owns a
// contiguous band of weights (Interface 30s, Agents 40s, Guardrails 50s,
// Devices 60s, Advanced 70s), so a conditional entry lands inside its group
// whenever it is shown. `group` is the display label of the nav header the
// entry sits under; only the Account / General basics at the top go without
// one. `subsections` groups several sections behind a single nav
// entry, surfaced as an in-pane segmented switch (see CUSTOMIZE_IDS below).
type NavItem = {
  id: SettingsSection;
  label: string;
  icon: IconName;
  order: number;
  group?: string;
  subsections?: SettingsSection[];
};

// Custom agents / Tools / Skills are all agent-building primitives — tools and
// skills exist mainly to be composed into agents — so they live behind one
// "Customize" nav entry with an in-pane segmented switch (see CustomizeSwitch).
// The nav entry's `id` is the default sub-tab.
const CUSTOMIZE_IDS: SettingsSection[] = ["agents", "tools", "skills"];

// The agent view itself: what appears around an agent and how you talk to it.
// Not "Workspace" — the title bar already calls agent sessions workspaces, and
// a Workspace group would read as settings about those.
const INTERFACE_GROUP = "Interface";
const AGENTS_GROUP = "Agents";
// Named for what the group is *for*, not the mechanism: what agents may touch
// (sandbox isolation, engine, container auth) and how their work is allowed to
// leave (approval wait, ask before publishing, draft PRs). Future permission,
// network or secrets settings belong here too. "Checkouts" or "Isolation" would
// read as plain worktree-running, which is exactly the framing to avoid.
const GUARDRAILS_GROUP = "Guardrails";
// Devices on both ends: phones paired to this host and other hosts this machine
// drives; cloud environments join here when they get settings of their own.
// Says nothing about the platform, so a Linux or Windows build needs no rename.
// One entry today — the header still earns its place by saying what Remote
// control is about, and by marking where the next device-side section goes.
const DEVICES_GROUP = "Devices";
// The conventional tail: things most people never open. A regular user sees
// "Advanced › Experimental" — a deliberate category, not a stray entry under
// Devices — and an admin's Developer slots in under the same header.
const ADVANCED_GROUP = "Advanced";

// General holds only what has no better home (appearance, alerts, privacy);
// every feature with more than a knob or two gets its own entry so its settings
// aren't buried in a long General page.
const NAV: NavItem[] = [
  { id: "account", label: "Account", icon: "user", order: 10 },
  { id: "general", label: "General", icon: "settings", order: 20 },
  { id: "layout", label: "Layout", icon: "panelGrid", order: 30, group: INTERFACE_GROUP },
  { id: "providers", label: "Providers", icon: "blocks", order: 40, group: AGENTS_GROUP },
  {
    id: "agents",
    label: "Customize",
    icon: "shapes",
    order: 43,
    group: AGENTS_GROUP,
    subsections: CUSTOMIZE_IDS,
  },
  // Sandbox leads the group so the header's meaning lands at a glance. The box
  // is the app's existing sandbox glyph (SandboxBadge, the env-vars sandbox
  // toggle), so the nav entry matches it.
  { id: "sandbox", label: "Sandbox", icon: "cube", order: 50, group: GUARDRAILS_GROUP },
  { id: "git", label: "Git", icon: "branch", order: 53, group: GUARDRAILS_GROUP },
  { id: "remote", label: "Remote control", icon: "phone", order: 60, group: DEVICES_GROUP },
  { id: "experimental", label: "Experimental", icon: "flask", order: 70, group: ADVANCED_GROUP },
];
// Stable sort by weight keeps contribution order on ties.
NAV.sort((a, b) => a.order - b.order);

// Developer is appended at render only when unlocked (dev build or admin user),
// so it slots by `order` among the base entries above.
const DEVELOPER_NAV: NavItem = {
  id: "developer",
  label: "Developer",
  icon: "wrench",
  order: 73,
  group: ADVANCED_GROUP,
};

// Dictation replaces Apple's recognizer, so there is nothing to configure on
// other platforms; the entry is added at render only on macOS. Its own entry
// rather than a group in Layout: besides the two toggles it owns the model
// choice and the weights download, which is a feature, not a display option.
const DICTATION_NAV: NavItem = {
  id: "dictation",
  label: "Dictation",
  icon: "mic",
  order: 33,
  group: INTERFACE_GROUP,
};

// Right after Customize — workflows chain those custom agents. Added at render
// because a host answers no `wf_*` op (docs/multi-host-plan.md §5.3, item 2):
// on a remote environment a run launched from here could not start, so the
// section is not offered. Everything else in Settings is about this desktop and
// stays put whatever environment is active.
const WORKFLOWS_NAV: NavItem = {
  id: "workflows",
  label: "Workflows",
  icon: "combine",
  order: 46,
  group: AGENTS_GROUP,
};

/** Dedicated full-screen settings surface. Rendered in place of the workspace
 *  panes while `settingsScreenOpen` is true. The quick-settings popover stays
 *  for fast access; this is the comprehensive surface. */
export function SettingsScreen() {
  const section = useAppStore((s) => s.settingsSection);
  const setSection = useAppStore((s) => s.setSettingsSection);
  const close = useAppStore((s) => s.closeSettingsScreen);
  const admin = useAppStore((s) => s.admin);
  const workflowGate = useGate("workflows");

  // Dev builds always expose Developer; production unlocks it only for admins.
  const showDeveloper = import.meta.env.DEV || admin;
  const nav = useMemo(
    () =>
      [
        ...NAV,
        ...(workflowGate ? [] : [WORKFLOWS_NAV]),
        ...(IS_MAC ? [DICTATION_NAV] : []),
        ...(showDeveloper ? [DEVELOPER_NAV] : []),
      ].sort((a, b) => a.order - b.order),
    [showDeveloper, workflowGate],
  );

  // A section with no nav entry (a stale "developer" after the admin flag
  // flipped off, or "dictation" off-Mac) falls back to General.
  const visible = nav.some((n) => n.id === section || n.subsections?.includes(section));

  return (
    <div className="set-screen">
      <nav className="set-nav">
        <button className="set-back flex-center text-base" onClick={close}>
          <Icon name="chevL" size={13} />
          <span>Back to app</span>
        </button>
        <div className="set-nav-list">
          {nav.map((n, i) => {
            // A header opens wherever the group changes between consecutive
            // visible entries, so a group with nothing visible never shows one.
            const opensGroup = !!n.group && n.group !== nav[i - 1]?.group;
            // A grouped entry stays active for any of its sub-sections, and
            // clicking it while already inside the group keeps the current
            // sub-tab rather than snapping back to the default.
            const active = n.subsections ? n.subsections.includes(section) : section === n.id;
            return (
              <Fragment key={n.id}>
                {opensGroup && <div className="set-nav-group mono text-xs">{n.group}</div>}
                <button
                  className={`set-nav-item flex-center text-base ${active ? "active" : ""}`}
                  onClick={() => {
                    if (active && n.subsections) return;
                    setSection(n.id);
                  }}
                >
                  <Icon name={n.icon} size={14} />
                  <span>{n.label}</span>
                </button>
              </Fragment>
            );
          })}
        </div>
        <div className="set-nav-foot text-xs">
          <span className="mono">Fletch</span>
          <span className="mono dim">v{pkg.version}</span>
        </div>
      </nav>

      <div className="set-main">
        <div className="set-content">
          {!visible && <GeneralPane />}
          {visible && section === "general" && <GeneralPane />}
          {visible && section === "account" && <AccountPane />}
          {visible && section === "layout" && <LayoutPane />}
          {visible && section === "dictation" && <DictationPane />}
          {visible && section === "git" && <GitPane />}
          {visible && section === "sandbox" && <SandboxPane />}
          {visible && section === "remote" && <RemoteControlPane />}
          {visible && section === "providers" && <ProvidersPane />}
          {visible && section === "agents" && <CustomAgentsPane />}
          {visible && section === "workflows" && <WorkflowsPane />}
          {visible && section === "skills" && <SkillsPane />}
          {visible && section === "tools" && <McpServersPane />}
          {visible && section === "experimental" && <ExperimentalPane />}
          {visible && section === "developer" && (
            <Suspense fallback={null}>
              <DeveloperPane />
            </Suspense>
          )}
        </div>
      </div>
    </div>
  );
}
