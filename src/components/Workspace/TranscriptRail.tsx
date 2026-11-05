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
import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { TranscriptList } from "./messages/TranscriptList";
import { useTranscript } from "./messages/useTranscript";
import { useLiveBusy } from "./useLiveBusy";

export function TranscriptRail({ agent, onClose }: { agent: AgentRecord; onClose: () => void }) {
  const transcript = useTranscript(agent);
  // A native turn can't pause on a question widget, so nothing to suppress.
  const liveBusy = useLiveBusy(agent, false);

  return (
    <aside className="native-rail" aria-label="Transcript">
      <div className="native-rail-h flex-center">
        <span className="native-rail-title text-sm">Transcript</span>
        <IconButton size="sm" tip="Hide transcript (⌘⇧T)" onClick={onClose}>
          <Icon name="close" />
        </IconButton>
      </div>
      <TranscriptList agent={agent} transcript={transcript} liveBusy={liveBusy} hideNav />
    </aside>
  );
}
