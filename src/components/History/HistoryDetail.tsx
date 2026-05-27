import { useEffect, useMemo, useRef, useState } from "react";
import { useAppStore } from "../../store";
import { applyPolicy, getAdapter } from "../../adapters";
import { Icon, LandmarkGlyph } from "../Icon";
import { IconButton } from "../ui/IconButton";
import { basename } from "../../util/format";
import { MessageItem } from "../Workspace/messages/MessageItem";
import { pairToolItems } from "../Workspace/messages/pair";

interface Props {
  agentId: string;
}

/** Detail view in History: reads the JSONL from disk and renders the
 *  conversation read-only, with a Restore button in the header. */
export function HistoryDetail({ agentId }: Props) {
  const workspace = useAppStore((s) => s.workspace);
  const log = useAppStore((s) => s.managedLogs[agentId]);
  const loadHistoryTranscript = useAppStore((s) => s.loadHistoryTranscript);
  const restore = useAppStore((s) => s.restore);
  const selectHistoryAgent = useAppStore((s) => s.selectHistoryAgent);
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);

  const [loading, setLoading] = useState(true);
  const [restoring, setRestoring] = useState(false);

  const agent = useMemo(
    () => workspace?.agents.find((a) => a.id === agentId) ?? null,
    [workspace?.agents, agentId],
  );

  // Replay the on-disk transcript into managedLogs once on mount.
  // The store reducer is the same one used for live stream-json
  // events, so we get the same MessageItem rendering for free.
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    loadHistoryTranscript(agentId).finally(() => {
      if (!cancelled) setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [agentId, loadHistoryTranscript]);

  const scrollRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [log]);

  if (!agent || !agent.archive) {
    return (
      <div className="pane center">
        <div className="empty-msg" style={{ margin: "auto", maxWidth: 320 }}>
          <div className="et">Session not found</div>
          <div>The agent record may have been removed.</div>
        </div>
      </div>
    );
  }

  const archive = agent.archive;
  const primary = archive.repos[0];
  const items = useMemo(() => {
    const adapter = getAdapter(agent.provider);
    const visible = applyPolicy(log ?? [], adapter.policy);
    return pairToolItems(visible);
  }, [log, agent.provider]);
  const transcriptEmpty = !loading && items.length === 0;

  const onRestore = async () => {
    setRestoring(true);
    try {
      await restore(agentId);
    } finally {
      setRestoring(false);
    }
  };

  const repoLabel = primary?.repo_path ? basename(primary.repo_path) : null;
  const headerTitle = repoLabel
    ? `${repoLabel} / ${agent.name}`
    : agent.name;

  return (
    <div className="pane center fade-in" key={agentId}>
      <div className="center-h">
        <IconButton
          tip={leftCollapsed ? "Show sidebar (⌘B)" : "Hide sidebar (⌘B)"}
          onClick={toggleLeft}
        >
          <Icon name="sidebarL" />
        </IconButton>

        <div className="task">
          <div className="t-name">
            <span className="hr-glyph">
              <LandmarkGlyph name={agent.name} />
            </span>
            <span>{headerTitle}</span>
          </div>
          <div className="t-meta">
            archived session
            {primary?.branch_name && <> · {primary.branch_name}</>}
          </div>
        </div>

        <button
          className="btn-t primary"
          onClick={onRestore}
          disabled={restoring}
        >
          <Icon name="archiveRestore" size={13} />
          <span>{restoring ? "Restoring…" : "Restore"}</span>
        </button>
      </div>

      <div className="chat history-body">
        <button
          className="history-back"
          onClick={() => selectHistoryAgent(null)}
        >
          <Icon name="chevL" size={12} />
          <span>Back to history</span>
        </button>
        <div className="chat-scroll" ref={scrollRef}>
          <div className="chat-inner fade-in">
            {loading ? (
              <div className="writing">
                <span className="dots">
                  <i /><i /><i />
                </span>
                <span>Loading transcript…</span>
              </div>
            ) : transcriptEmpty ? (
              <div className="empty-msg" style={{ margin: "40px auto", maxWidth: 360 }}>
                <div className="et">No transcript available</div>
                <div>
                  Claude's session file isn't on disk for this agent. You can
                  still restore the worktree and branch — the conversation will
                  start fresh.
                </div>
              </div>
            ) : (
              items.map((item, i) => <MessageItem key={i} item={item} />)
            )}
          </div>
        </div>
        <div className="history-detail-foot">
          <span>Read-only preview · restore to continue this session</span>
        </div>
      </div>
    </div>
  );
}
