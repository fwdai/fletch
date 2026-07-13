// run/eventSummary.ts — the timeline reducer. Maps a journal event (spec §7.1)
// to a product-language one-liner for the Run Monitor timeline. The raw payload
// JSON is shown only behind an expand affordance in the Timeline component
// (spec §14.2 guardrail) — this module never stringifies it into the summary.
//
// Payloads arrive parsed but untyped (`unknown`), and later slices (S5/S9–S12)
// attach richer fields, so every read is defensive: an unknown type or a missing
// field degrades to a sensible humanized label rather than throwing.

import type { WfEvent, WfPausedReason } from "../../api";
import type { IconName } from "../../components/Icon";
import { pausedLabel } from "./status";

const GREEN = "oklch(0.72 0.15 150)";
const AMBER = "oklch(0.78 0.14 75)";
const ACCENT = "var(--accent)";
const DANGER = "var(--danger)";
const MUTED = "var(--fg-3)";

export interface EventSummary {
  icon: IconName;
  tone: string;
  /** The one-line, product-language headline shown on the timeline row. */
  title: string;
  /** Optional secondary line (e.g. a gate's reason, an error message). */
  detail?: string;
}

/** Read a string field from an untyped payload, or undefined. */
function str(payload: unknown, key: string): string | undefined {
  if (payload && typeof payload === "object" && key in payload) {
    const v = (payload as Record<string, unknown>)[key];
    if (typeof v === "string") return v;
    if (typeof v === "number") return String(v);
  }
  return undefined;
}

/** Short 7-char form of a commit sha for display. */
function short(sha: string | undefined): string {
  return sha ? sha.slice(0, 7) : "";
}

/** Humanize an event type we don't have a bespoke summary for. */
function humanize(type: string): string {
  return type.replace(/_/g, " ").replace(/^\w/, (c) => c.toUpperCase());
}

export function summarizeEvent(ev: WfEvent): EventSummary {
  const p = ev.payload;
  switch (ev.type) {
    case "run_launched":
      return { icon: "play", tone: ACCENT, title: "Run launched" };
    case "run_resumed":
      return { icon: "play", tone: ACCENT, title: "Run resumed" };
    case "run_paused": {
      const reason = str(p, "reason") as WfPausedReason | undefined;
      return {
        icon: "pause",
        tone: AMBER,
        title: `Paused — ${reason ? pausedLabel(reason) : "paused"}`,
        detail: str(p, "detail"),
      };
    }
    case "run_done":
      return { icon: "check", tone: GREEN, title: "Run complete" };
    case "run_failed":
      return { icon: "close", tone: DANGER, title: "Run failed", detail: str(p, "error") };
    case "run_canceled":
      return { icon: "stop", tone: MUTED, title: "Run canceled" };

    case "attempt_spawned":
      return { icon: "bot", tone: MUTED, title: "Agent spawned" };
    case "skills_missing":
      return {
        icon: "minus",
        tone: AMBER,
        title: "Started without missing skills",
        detail: strList(p, "skills"),
      };
    case "custom_agent_missing":
      return {
        icon: "minus",
        tone: AMBER,
        title: "Custom agent no longer exists — started without its skills and MCP servers",
        detail: str(p, "custom_agent"),
      };
    case "attempt_ready":
      return { icon: "dot", tone: MUTED, title: "Agent ready" };
    case "prompt_sent": {
      const kind = str(p, "kind") ?? "step";
      const title =
        kind === "nudge"
          ? "Nudge sent"
          : kind === "reprompt"
            ? "Re-prompted"
            : kind === "message"
              ? "Message delivered"
              : "Prompt sent";
      return { icon: "arrowUp", tone: MUTED, title };
    }
    case "turn_ended": {
      const status = str(p, "status");
      return {
        icon: status === "error" ? "close" : "dot",
        tone: status === "error" ? DANGER : MUTED,
        title: status === "error" ? "Turn ended with an error" : "Turn ended",
      };
    }
    case "gate_evaluated": {
      const mode = str(p, "mode") ?? "gate";
      const outcome = str(p, "outcome") ?? "";
      const reason = str(p, "reason");
      if (outcome === "done") {
        return { icon: "check", tone: GREEN, title: `Gate \`${mode}\` passed`, detail: reason };
      }
      const word = outcome === "awaiting_approval" ? "needs approval" : outcome || "unmet";
      return { icon: "minus", tone: AMBER, title: `Gate \`${mode}\` — ${word}`, detail: reason };
    }
    case "boundary_commit":
      return { icon: "commit", tone: GREEN, title: `Committed ${short(str(p, "sha"))}` };

    case "attempt_abandoned":
      return {
        icon: "minus",
        tone: MUTED,
        title: "Attempt abandoned",
        detail: str(p, "cause"),
      };
    case "attempt_error":
      return { icon: "close", tone: DANGER, title: "Attempt error", detail: str(p, "error") };
    case "watchdog_stalled":
      return { icon: "clock", tone: AMBER, title: "No activity — nudging the agent" };

    case "budget_tick":
      return { icon: "zap", tone: MUTED, title: "Budget updated" };
    case "budget_exceeded":
      return {
        icon: "zap",
        tone: AMBER,
        title: `Budget reached${str(p, "which") ? ` — ${str(p, "which")}` : ""}`,
      };

    case "message_routed": {
      const kind = str(p, "kind") ?? "message";
      return { icon: "inbox", tone: ACCENT, title: `Message routed — ${kind}` };
    }
    case "decision":
      return {
        icon: "sparkle",
        tone: ACCENT,
        title: `Decision — ${str(p, "decision") ?? "issued"}`,
      };
    case "child_spawn_requested":
      return { icon: "combine", tone: MUTED, title: "Child spawn requested" };
    case "child_spawn_approved":
      return { icon: "combine", tone: GREEN, title: "Child spawn approved" };
    case "child_spawn_denied":
      return {
        icon: "combine",
        tone: AMBER,
        title: "Child spawn denied",
        detail: str(p, "reason"),
      };
    case "subrun_launched":
      return { icon: "combine", tone: ACCENT, title: "Sub-run launched" };
    case "subrun_finished":
      return {
        icon: "combine",
        tone: MUTED,
        title: `Sub-run finished${str(p, "status") ? ` — ${str(p, "status")}` : ""}`,
      };

    case "merge_started":
      return { icon: "merge", tone: ACCENT, title: "Merging children" };
    case "merge_conflict":
      return { icon: "close", tone: DANGER, title: "Merge conflict", detail: strList(p, "files") };
    case "merge_done":
      return { icon: "merge", tone: GREEN, title: `Merged ${short(str(p, "sha"))}` };

    case "finalize_pushed":
      return {
        icon: "push",
        tone: GREEN,
        title: `Pushed ${str(p, "branch") ?? "branch"}`,
      };
    case "finalize_pr": {
      const err = str(p, "error");
      if (err) return { icon: "pr", tone: DANGER, title: "PR failed", detail: err };
      return { icon: "pr", tone: GREEN, title: "Pull request opened", detail: str(p, "url") };
    }

    default:
      return { icon: "more", tone: MUTED, title: humanize(ev.type) };
  }
}

/** A payload's string-array field rendered as a short detail line. */
function strList(payload: unknown, key: string): string | undefined {
  if (payload && typeof payload === "object" && key in payload) {
    const items = (payload as Record<string, unknown>)[key];
    if (Array.isArray(items) && items.length > 0) {
      return items.filter((f) => typeof f === "string").join(", ");
    }
  }
  return undefined;
}
