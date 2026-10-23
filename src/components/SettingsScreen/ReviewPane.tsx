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
        desc="A second agent reads every change before it ships, in the workspace itself — no GitHub needed. It runs alongside GitHub reviewers like Greptile; its findings land in the Git panel's Review row, where the coding agent picks them up."
      />

      <SetGroup label="When">
        <SetRow
          title="Review on PR open"
          sub="Run the reviewer the moment a pull request opens, and again after the agent addresses its findings. You can always ask for a review earlier from the Git panel."
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
              ? `${effective.name} reviews on ${effective.base}${effective.model ? ` · ${effective.model}` : ""}. Its brief says what to look for.`
              : "No custom agent is set up to review yet. Create the Reviewer, or pick any custom agent."
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
              ? "Tell it what matters in this codebase: what to flag, what to leave alone."
              : "Seeds the starter pack's Reviewer: a rigorous, file-and-line code review brief."
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
