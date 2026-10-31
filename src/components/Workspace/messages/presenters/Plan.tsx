// The agent's working plan: Claude's `TodoWrite` (a whole checklist each call)
// and Codex's `update_plan` (the same, as `plan: [{ step, status }]`). The row
// says how far along the plan is and what's in hand; opened, it's the list.

import { Icon } from "@/components/Icon";
import type { StepFacts, ToolPresenter } from "./types";
import { getStringField } from "./util";

type Status = "pending" | "in_progress" | "completed";

interface PlanItem {
  text: string;
  status: Status;
}

function asStatus(v: unknown): Status {
  return v === "completed" || v === "in_progress" ? v : "pending";
}

/** The plan's items, from either shape. */
function planItems(input: unknown): PlanItem[] {
  if (!input || typeof input !== "object") return [];
  const bag = input as Record<string, unknown>;
  const raw = Array.isArray(bag.todos) ? bag.todos : Array.isArray(bag.plan) ? bag.plan : [];
  return raw
    .filter((e): e is Record<string, unknown> => !!e && typeof e === "object")
    .map((e) => ({
      text: getStringField(e, "content") || getStringField(e, "step"),
      status: asStatus(e.status),
    }))
    .filter((e) => e.text);
}

/** What's being worked on: the in-progress item's present-tense form when the
 *  agent gave one (`activeForm`), else its text. */
function current(input: unknown): string {
  if (!input || typeof input !== "object") return "";
  const bag = input as Record<string, unknown>;
  const raw = Array.isArray(bag.todos) ? bag.todos : Array.isArray(bag.plan) ? bag.plan : [];
  for (const e of raw) {
    if (e && typeof e === "object" && (e as Record<string, unknown>).status === "in_progress") {
      return (
        getStringField(e, "activeForm") || getStringField(e, "content") || getStringField(e, "step")
      );
    }
  }
  return "";
}

const PLAN_FACTS: StepFacts = { kind: "plan" };

export const planPresenter: ToolPresenter = {
  icon: "task",
  title: "Plan",
  summary: (call) => {
    const items = planItems(call.input);
    const done = items.filter((i) => i.status === "completed").length;
    const now = current(call.input);
    return (
      <>
        {items.length > 0 && (
          <span className="summary-lead">
            {done}/{items.length}
          </span>
        )}
        <span className="t-prose">
          {now || (items.length > 0 && done === items.length ? "All done" : "Updated")}
        </span>
      </>
    );
  },
  expanded: (call) => {
    const items = planItems(call.input);
    const explanation = getStringField(call.input, "explanation");
    return (
      <>
        {explanation && <div className="tb-cap">{explanation}</div>}
        <ul className="tb-plan">
          {items.map((item, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: plan items have no ids; their order is fixed
            <li key={i} className={item.status}>
              <span className="tb-plan-mark" aria-hidden>
                {item.status === "completed" ? (
                  <Icon name="check" size={11} />
                ) : item.status === "in_progress" ? (
                  <i />
                ) : null}
              </span>
              <span>{item.text}</span>
            </li>
          ))}
        </ul>
      </>
    );
  },
  facts: () => PLAN_FACTS,
  activity: () => "Updating the plan",
};
