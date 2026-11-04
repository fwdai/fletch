// run/eventSummary.ts — the timeline reducer. Maps a journal event (spec §7.1)
// to a product-language one-liner for the Run Monitor timeline. The raw payload
// JSON is shown only behind an expand affordance in the Timeline component
// (spec §14.2 guardrail) — this module never stringifies it into the summary.
//
// Payloads arrive parsed but untyped (`unknown`), and old journals still hold
// event types the kernel never writes, so every read is defensive: an unknown
// type or a missing field degrades to a sensible humanized label rather than
// throwing.

import type { WfEvent } from "../../api";
import type { IconName } from "../../components/Icon";
import { AMBER, GREEN } from "./status";

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
    case "mcp_servers_missing":
      return {
        icon: "minus",
        tone: AMBER,
        title: "Started without missing MCP servers",
        detail: strList(p, "mcp_servers"),
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
    case "prompt_sent":
      return { icon: "arrowUp", tone: MUTED, title: "Prompt sent" };
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
      return {
        icon: "minus",
        tone: AMBER,
        title: `Gate \`${mode}\` — ${outcome || "unmet"}`,
        detail: reason,
      };
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
