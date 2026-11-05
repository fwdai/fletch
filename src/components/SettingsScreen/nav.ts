// The settings nav as data: which sections exist, what each is called, and
// where it sits. Shared by the settings screen (which draws it) and the
// command palette (which offers every section as a "Settings › X" row), so a
// section cannot appear in one and not the other.

import type { IconName } from "@/components/Icon";
import type { SettingsSection } from "@/storage/preferences";

// Built-in sections carry explicit order weights so extension panes can slot
// *between* them via their own `order`, not just append. Each `group` owns a
// contiguous band of weights (Interface 30s, Agents 40s, Guardrails 50s,
// Devices 60s, Advanced 70s), so a conditional entry lands inside its group
// whenever it is shown. `group` is the display label of the nav header the
// entry sits under; only the Account / General basics at the top go without
// one. `subsections` groups several sections behind a single nav
// entry, surfaced as an in-pane segmented switch (see CUSTOMIZE_IDS below).
export type NavItem = {
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

// Network sits under Sandbox rather than beside it: it is a property of the VM,
// and neither the app nor the macOS sandbox limits what it can reach.
const SANDBOX_IDS: SettingsSection[] = ["sandbox", "network"];

// The agent view itself: what appears around an agent and how you talk to it.
// Not "Workspace" — the title bar already calls agent sessions workspaces, and
// a Workspace group would read as settings about those.
const INTERFACE_GROUP = "Interface";
const AGENTS_GROUP = "Agents";
// Named for what the group is *for*, not the mechanism: what agents may touch
// (sandbox isolation) and how their work is allowed to
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
  { id: "shortcuts", label: "Shortcuts", icon: "keyboard", order: 36, group: INTERFACE_GROUP },
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
  // is the app's existing sandbox glyph (the title bar's box workspaces, the
  // env-vars sandbox toggle), so the nav entry matches it.
  {
    id: "sandbox",
    label: "Sandbox",
    icon: "cube",
    order: 50,
    group: GUARDRAILS_GROUP,
    subsections: SANDBOX_IDS,
  },
  { id: "git", label: "Git", icon: "branch", order: 53, group: GUARDRAILS_GROUP },
  // Beside Git: what leaves a workspace is reviewed before it does.
  { id: "review", label: "Review", icon: "search", order: 56, group: GUARDRAILS_GROUP },
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

/** The nav entries shown on this build and platform, in display order.
 *  `workflows` is offered only where the host runs them (the `workflows` gate),
 *  `developer` on dev builds and for admins, `dictation` on macOS. */
export function settingsNav(opts: {
  workflows: boolean;
  mac: boolean;
  developer: boolean;
}): NavItem[] {
  return [
    ...NAV,
    ...(opts.workflows ? [WORKFLOWS_NAV] : []),
    ...(opts.mac ? [DICTATION_NAV] : []),
    ...(opts.developer ? [DEVELOPER_NAV] : []),
  ].sort((a, b) => a.order - b.order);
}

/** A section the palette can jump to: a nav entry, or one of the sub-tabs a
 *  grouped entry folds in (Tools and Skills under Customize, Network under
 *  Sandbox). The sub-tab labels mirror `CustomizeSwitch` / `SandboxSwitch`. */
export interface SettingsDestination {
  id: SettingsSection;
  label: string;
  icon: IconName;
  /** The nav header, or the grouped entry a sub-tab sits under. */
  group?: string;
}

const SUBSECTIONS: Partial<Record<SettingsSection, { label: string; icon: IconName }>> = {
  tools: { label: "Tools", icon: "webhook" },
  skills: { label: "Skills", icon: "notebookPen" },
  network: { label: "Network", icon: "globe" },
};

/** Every section reachable on this build, sub-tabs included, in nav order. */
export function settingsDestinations(opts: {
  workflows: boolean;
  mac: boolean;
  developer: boolean;
}): SettingsDestination[] {
  return settingsNav(opts).flatMap((n) => {
    const own: SettingsDestination = { id: n.id, label: n.label, icon: n.icon, group: n.group };
    const subs = (n.subsections ?? [])
      .filter((id) => id !== n.id && SUBSECTIONS[id])
      .map((id): SettingsDestination => ({ id, ...SUBSECTIONS[id]!, group: n.label }));
    return [own, ...subs];
  });
}
