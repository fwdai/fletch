// Structured transcript beside the native view's terminal.
//
// The TUI is the live surface; this is the inspector. It renders the same
// structured log the custom view does — tool cards, thinking blocks, turn
// footers, fork-from-here — sourced from the agent's own on-disk transcript
// rather than from an event stream the PTY doesn't have.
//
// Granularity note (measured, see the live-sync poller in supervisor/): the
// CLI flushes one complete assistant message plus its tool result at a time,
// so tool activity appears within about a second, but a turn's final prose
// lands atomically when the turn ends. This surface is therefore honest as
// "structured progress", not as a mirror of the text typing out in the TUI.
import { hasUsage } from "@/adapters/usage";
import type { AgentRecord } from "@/api";
import { UsageMeter } from "@/components/Composer/UsageMeter";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { TranscriptList } from "./messages/TranscriptList";
import { useTranscript } from "./messages/useTranscript";

export function TranscriptRail({ agent, onClose }: { agent: AgentRecord; onClose: () => void }) {
  const transcript = useTranscript(agent);
  // Native turns produce no event stream, so `managedBusy` is never set for
  // them. The agent's own status is the liveness signal here.
  const liveBusy = agent.status === "running";

  return (
    <aside className="native-rail" aria-label="Transcript">
      <div className="native-rail-h flex-center">
        <span className="native-rail-title text-sm">Transcript</span>
        <IconButton size="sm" tip="Hide transcript (⌘⇧T)" onClick={onClose}>
          <Icon name="close" />
        </IconButton>
      </div>
      <TranscriptList agent={agent} transcript={transcript} liveBusy={liveBusy} hideNav />
      <UsageFoot agentId={agent.id} />
    </aside>
  );
}

/** Context/token usage for the session. The custom view shows this in the
 *  composer foot, which native has no equivalent of — so without it here, cost
 *  stays invisible in native even though the store already has it (usage is
 *  folded from the same transcript records this rail renders). */
function UsageFoot({ agentId }: { agentId: string }) {
  const usage = useAppStore((s) => s.usage[agentId]);
  const show = useAppStore((s) => s.features.tokenUsage);
  if (!show || !usage || !hasUsage(usage)) return null;
  return (
    <div className="native-rail-foot flex-center">
      <UsageMeter usage={usage} />
    </div>
  );
}
