import { useMemo } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { Select, type SelectOption } from "@/components/ui/Select";
import { REVIEWER_PRESET } from "@/starterPack";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { REVIEWER_NAME, resolveReviewer } from "@/store/review";
import { Mono } from "./CustomAgents/Mono";
import { SetGroup, SetHead, SetRow, SetToggle } from "./primitives";

/** The `review.agent` value for "let the host pick": the agent named
 *  Reviewer, which the starter pack seeds. */
const DEFAULT_PICK = "";

/** The local reviewer: whether it runs when a PR opens, and which custom
 *  agent does the reviewing. The agent is the whole brief — what it pays
 *  attention to — so editing it is the way to shape reviews. */
export function ReviewPane() {
  const onPrOpen = useAppStore((s) => s.reviewOnPrOpen);
  const setOnPrOpen = useAppStore((s) => s.setReviewOnPrOpen);
  const pickedId = useAppStore((s) => s.reviewAgentId);
  const setReviewAgent = useAppStore((s) => s.setReviewAgent);
  const agents = useAppStore((s) => s.customAgents);
  const createCustomAgent = useAppStore((s) => s.createCustomAgent);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  const setLastError = useAppStore((s) => s.setLastError);
  // The loop runs on the host, so a host from before it has nothing to set.
  const gate = useGate("review");
  const tip = gate ?? undefined;

  const effective = useMemo(() => resolveReviewer(agents, pickedId), [agents, pickedId]);
  const seeded = agents.some((a) => a.name.trim().toLowerCase() === REVIEWER_NAME.toLowerCase());
  // The picked id, or the default when it points nowhere (a deleted agent).
  const value = agents.some((a) => a.id === pickedId) ? pickedId : DEFAULT_PICK;

  const options: SelectOption<string>[] = [
    {
      value: DEFAULT_PICK,
      label: seeded ? `${REVIEWER_NAME} (default)` : "None yet",
      hint: seeded ? "the starter pack's reviewer" : "create the Reviewer below",
    },
    ...agents.map((a) => ({
      value: a.id,
      label: a.name,
      hint: a.base,
      icon: <Mono name={a.name} hue={a.color} size={16} />,
    })),
  ];

  const createReviewer = async () => {
    try {
      const created = await createCustomAgent(REVIEWER_PRESET);
      await setReviewAgent(created.id);
    } catch (e) {
      setLastError(`Couldn't create the Reviewer: ${e}`);
    }
  };

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Review"
        title="Review"
        desc="How a second agent checks changes before they ship."
      />

      <SetGroup label="When">
        <SetRow
          title="Review on PR open"
          sub={
            effective
              ? "Review each PR when it opens, and again after the agent addresses findings."
              : "Waits for a reviewer. Add one below and PRs get reviewed as they open."
          }
        >
          <SetToggle
            on={onPrOpen}
            onClick={() => void setOnPrOpen(!onPrOpen)}
            disabled={!!gate}
            tip={tip}
          />
        </SetRow>
      </SetGroup>

      <SetGroup label="Who" last>
        <SetRow
          title="Reviewer"
          sub={
            effective
              ? `${effective.name} reviews on ${effective.base}${effective.model ? ` · ${effective.model}` : ""}.`
              : "No reviewer yet. Create the Reviewer, or pick any custom agent."
          }
        >
          <Select<string>
            value={value}
            ariaLabel="Reviewer"
            options={options}
            disabled={!!gate}
            onChange={(v) => void setReviewAgent(v)}
          />
        </SetRow>
        <SetRow
          title={effective ? `Edit ${effective.name}'s brief` : "Create the Reviewer"}
          sub={
            effective
              ? "Tell it what to flag and what to leave alone."
              : "Add the starter pack's Reviewer agent."
          }
        >
          {effective ? (
            <Button
              variant="outline"
              size="sm"
              onClick={() => openSettingsScreen("agents", `edit-custom-agent:${effective.id}`)}
            >
              <Icon name="edit" size={12} /> Edit brief
            </Button>
          ) : (
            <Button variant="primary" size="sm" onClick={() => void createReviewer()}>
              <Icon name="plus" size={12} /> Create Reviewer
            </Button>
          )}
        </SetRow>
      </SetGroup>
    </div>
  );
}
