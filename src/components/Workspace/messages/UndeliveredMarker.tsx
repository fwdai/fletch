import { Button } from "@/components/ui/Button";
import { UNDELIVERED_LABEL } from "@/helpers/mergeTurns";
import { useAppStore } from "@/store";

/** The foot of a user bubble the agent never read (see
 *  `ChatItem.undelivered`): why, and a Resend that sends the same text and
 *  attachments as a new turn. The bubble stays put as the record of what
 *  happened; the resent message draws its own. */
export function UndeliveredMarker({
  reason,
  agentId,
  text,
  attachments,
}: {
  reason: "interrupted" | "failed";
  agentId?: string;
  text: string;
  attachments?: string[];
}) {
  const send = useAppStore((s) => s.sendUserMessage);
  return (
    <div className="m-user__undelivered text-xs">
      <span>{UNDELIVERED_LABEL[reason]}</span>
      {agentId && (
        <Button variant="link" size="sm" onClick={() => void send(agentId, text, attachments)}>
          Resend
        </Button>
      )}
    </div>
  );
}
