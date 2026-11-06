// The things to jump to: agents, drafts, runs, projects, settings sections,
// archived agents and environments. Each is a row that selects or opens it.

import type { AgentRecord } from "@/api";
import { settingsDestinations } from "@/components/SettingsScreen/nav";
import { agentDotStatus, STATUS_LABEL } from "@/components/TitleBar/WorkspaceStatus/derive";
import { activeEntry, connectionLabel } from "@/store/capabilities";
import type { EnvironmentEntry } from "@/store/environments";
import { gateReason } from "@/store/gates";
import type { AppState } from "@/store/types";
import { basename, firstLine, formatAge } from "@/util/format";
import { IS_MAC } from "@/util/platform";
import {
  agentTitle,
  byAttention,
  go,
  type PaletteContext,
  projectName,
  SEC,
  sidebarAgents,
} from "./context";
import type { PaletteItem } from "./types";

function agentRow(ctx: PaletteContext, a: AgentRecord): PaletteItem {
  const { s } = ctx;
  const status = agentDotStatus(a.status, s.pendingToolUse[a.id]);
  const repo = a.repos[0]?.repo_path;
  const branch = a.repos[0]?.branch;
  return {
    id: `agent:${a.id}`,
    label: agentTitle(a),
    hint: [projectName(s, repo), branch].filter(Boolean).join(" · "),
    meta: STATUS_LABEL[status],
    leading: { kind: "status", status },
    section: SEC.agents,
    keywords: [a.name, firstLine(a.task, 120), branch, projectName(s, repo)]
      .filter(Boolean)
      .join("  "),
    run: () => go(s, () => s.selectAgent(a.id)),
  };
}

export function environmentRow(s: AppState, e: EnvironmentEntry): PaletteItem {
  const current = e.id === s.activeEnvironmentId;
  return {
    id: `env:${e.id}`,
    label: e.name,
    hint: connectionLabel(e),
    meta: current ? "Current" : undefined,
    leading: { kind: "icon", name: e.kind === "local" ? "laptop" : "globe" },
    section: SEC.environments,
    keywords: "environment  host  switch  machine",
    disabled: current ? "Already here" : null,
    run: () => void s.switchEnvironment(e.id),
  };
}

/** This Mac first — it is always there — then the hosts by name, so the list
 *  doesn't reshuffle as connections come and go (as `EnvironmentMenu`). */
export function environmentRows(s: AppState): PaletteItem[] {
  const order = (a: EnvironmentEntry, b: EnvironmentEntry) =>
    a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "local" ? -1 : 1;
  return Object.values(s.environments)
    .sort(order)
    .map((e) => environmentRow(s, e));
}

/** Every settings section on this build, sub-tabs included. `prefix` is what
 *  the root search shows ("Settings › X"); the settings page drops it. */
export function settingsRows(ctx: PaletteContext, prefix: string): PaletteItem[] {
  const { s } = ctx;
  const nav = settingsDestinations({
    workflows: !gateReason(activeEntry(s), "workflows"),
    mac: IS_MAC,
    developer: ctx.developer,
  });
  return nav.map((n) => ({
    id: `settings:${n.id}`,
    label: `${prefix}${n.label}`,
    hint: n.group,
    leading: { kind: "icon", name: n.icon },
    section: SEC.settings,
    keywords: `settings  preferences  ${n.id}`,
    run: () => go(s, () => s.openSettingsScreen(n.id)),
  }));
}

/** The projects a draft can start in, the one an agent last started in first. */
export function projectRows(s: AppState, run: (repoPath: string) => void): PaletteItem[] {
  const projects = [...(s.workspace?.projects ?? [])].sort((a, b) =>
    a.path === s.lastRepoPath ? -1 : b.path === s.lastRepoPath ? 1 : 0,
  );
  return projects.map((p) => ({
    id: `pick:${p.path}`,
    label: p.name,
    hint: p.path,
    meta: p.path === s.lastRepoPath ? "Last used" : undefined,
    leading: { kind: "image", project: p },
    section: SEC.projects,
    keywords: basename(p.path),
    run: () => run(p.path),
  }));
}

/** Everything the root search can jump to. */
export function entityItems(ctx: PaletteContext): PaletteItem[] {
  const { s } = ctx;
  const items: PaletteItem[] = [];
  const active = activeEntry(s);

  for (const a of byAttention(s, sidebarAgents(s))) items.push(agentRow(ctx, a));

  for (const d of s.drafts) {
    if (!s.workspace?.projects.some((p) => p.path === d.repoPath)) continue;
    items.push({
      id: `draft:${d.id}`,
      label: d.name,
      hint: `${d.kind === "workflow" ? "Workflow" : "Agent"} draft · ${projectName(s, d.repoPath)}`,
      leading: { kind: "icon", name: d.kind === "workflow" ? "combine" : "edit" },
      section: SEC.drafts,
      keywords: "draft  new  unsent",
      run: () => go(s, () => s.selectDraft(d.id)),
    });
  }

  for (const r of ctx.runs) {
    items.push({
      id: `run:${r.id}`,
      label: r.name,
      hint: [projectName(s, r.repo_path), firstLine(r.task)].filter(Boolean).join(" · "),
      meta: r.status,
      leading: { kind: "icon", name: "combine" },
      section: SEC.runs,
      keywords: `workflow  run  ${r.branch}  ${r.status}`,
      run: () => go(s, () => s.selectRun(r.id)),
    });
  }

  const roadmapGate = gateReason(active, "roadmap");
  for (const p of s.workspace?.projects ?? []) {
    items.push({
      id: `project:${p.path}`,
      label: `Roadmap · ${p.name}`,
      hint: p.path,
      leading: { kind: "image", project: p },
      section: SEC.projects,
      keywords: `project  repo  board  ${basename(p.path)}`,
      disabled: roadmapGate,
      run: () => go(s, () => s.openRoadmap(p.path)),
    });
  }

  items.push(...settingsRows(ctx, "Settings › "));

  const restoreGate = gateReason(active, "restore");
  const archived = (s.workspace?.agents ?? [])
    .filter((a) => a.archive)
    .sort((a, b) => ((a.archive?.archived_at ?? "") < (b.archive?.archived_at ?? "") ? 1 : -1));
  for (const a of archived) {
    const age = a.archive ? formatAge(a.archive.archived_at, ctx.now) : "";
    const when = age === "now" ? "just now" : age ? `${age} ago` : "";
    items.push({
      id: `restore:${a.id}`,
      label: agentTitle(a),
      hint: [projectName(s, a.repos[0]?.repo_path), when && `archived ${when}`]
        .filter(Boolean)
        .join(" · "),
      meta: "Restore",
      leading: { kind: "icon", name: "archiveRestore" },
      section: SEC.history,
      keywords: [a.name, firstLine(a.task, 120), a.repos[0]?.branch, "archived  history  restore"]
        .filter(Boolean)
        .join("  "),
      disabled: restoreGate,
      // The restore selects the agent once it lands; leave the takeovers now.
      run: () => go(s, () => void s.restore(a.id)),
    });
  }

  if (Object.keys(s.environments).length > 1) items.push(...environmentRows(s));
  return items;
}
