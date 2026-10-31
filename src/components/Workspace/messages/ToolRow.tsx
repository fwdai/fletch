import { type ReactNode, useRef } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { Loader } from "@/components/ui/Loader";
import { DisclosureBody, useDisclosure, useRevealedText } from "./Disclosure";
import { useChatFocus } from "./useChatFocus";

/** Shared chrome for every tool presenter: icon, name, one-line summary,
 *  click-to-expand. Presenters supply the summary and expanded bodies.
 *  `running` marks a tool call still in flight (no result yet, agent busy)
 *  so the row shows a live spinner instead of looking identical to a settled
 *  one. `toolUseId` + `agentId` let the row answer a `chatFocus` request: it
 *  opens, scrolls itself into view, and clears the request (useChatFocus).
 *  `revealText` (lower-cased) opens the row while a find query matches it. */
export function ToolRow({
  name,
  icon = "wrench",
  isError,
  running,
  summary,
  expanded,
  toolUseId,
  agentId,
  revealText,
}: {
  name: string;
  icon?: IconName;
  isError?: boolean;
  running?: boolean;
  summary: ReactNode;
  expanded: ReactNode;
  toolUseId?: string;
  agentId?: string;
  revealText?: string;
}) {
  const { open, instant, reveal, toggle } = useDisclosure();
  const revealed = useRevealedText(revealText);
  const shown = open || revealed;
  const rootRef = useRef<HTMLDivElement | null>(null);
  useChatFocus(agentId, toolUseId, () => rootRef.current, reveal);

  const cls = ["tool-row", shown && "is-open", isError && "is-error"].filter(Boolean).join(" ");
  return (
    <div ref={rootRef} className={cls} data-tool-use-id={toolUseId}>
      <button type="button" className="m-tool flex-center" aria-expanded={shown} onClick={toggle}>
        <Icon name={icon} size={12} className="t-icon" />
        <span className="t-name">{name}</span>
        <span className="t-arg">{summary}</span>
        {running ? (
          <Loader variant="muted" size="sm" aria-label={`${name} running`} />
        ) : (
          <Icon name="chevR" size={12} className="t-chev" />
        )}
      </button>
      <DisclosureBody open={shown} instant={instant || (revealed && !open)} className="tool-detail">
        <div className="tool-body">{expanded}</div>
      </DisclosureBody>
    </div>
  );
}
