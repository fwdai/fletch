import { useState } from "react";
import { AgentIdentityChip } from "@/components/AgentIdentityChip";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { Loader } from "@/components/ui/Loader";
import type { RoadmapState } from "../useRoadmap";
import { ChatPane } from "./ChatPane";
import { ChatPicker } from "./ChatPicker";
import { NewChatScreen } from "./NewChatScreen";
import { type ChatAgentPick, usePmChats } from "./usePmChats";

/** The left column: a real conversation with the project's PM agent.
 *
 *  Each chat is its own workspace, so the agent can read the codebase it
 *  reasons about — and each is deliberately disposable: start a new one
 *  whenever the subject changes rather than letting a single thread grow until
 *  its context is worthless. The agent cannot edit or publish code (the backend
 *  denies it the publish ops); the only thing it can put on the board is a
 *  proposal the user accepts. */
export function Thread({ roadmap, repoPath }: { roadmap: RoadmapState; repoPath: string }) {
  const { projectId } = roadmap;
  const chats = usePmChats(projectId, repoPath);
  const { selected } = chats;
  /** The user asked for the new-chat screen. With no chats yet that screen is
   *  the column's only body anyway — see `newChat` below — so this flag is only
   *  what makes it *replace* a conversation. */
  const [composing, setComposing] = useState(false);

  // The screen stays up for the whole spawn — it shows its own "starting" state,
  // and closing it first would flash the *previous* conversation back on screen
  // for as long as the spawn takes. A failed spawn leaves it up with the error.
  const start = async (pick: ChatAgentPick, firstMessage?: string) => {
    if (await chats.startChat(pick, firstMessage)) setComposing(false);
  };

  // The new-chat screen owns the body whenever the user asked for it, and
  // whenever there is no conversation to show instead.
  const newChat = !chats.loading && !!projectId && (composing || !selected);

  return (
    <section className="rm-thread">
      <div className="rm-thread-head flex-center">
        {/* A live chat wears its agent's identity; the session screen and the
            empty column wear the same planning mark the screen leads with, so
            the header and the body below it read as one thing. */}
        {selected && !newChat ? (
          <AgentIdentityChip agent={selected} size={20} />
        ) : (
          <span className="rm-pm-badge iflex-center">
            <Icon name="notebookPen" size={12} />
          </span>
        )}

        {newChat ? (
          <span className="rm-pm-n text-sm">New planning session</span>
        ) : selected ? (
          <ChatPicker
            chats={chats.chats}
            selected={selected}
            onSelect={chats.select}
            onNew={() => setComposing(true)}
            onDelete={(id) => void chats.deleteChat(id)}
          />
        ) : (
          <span className="rm-pm-n text-sm">Project manager</span>
        )}

        <span className="grow" />

        {/* One affordance, two directions: open the screen, or leave it for the
            chat you came from. Absent while the screen is the only body there
            is — the project's first chat has nothing to go back to. */}
        {selected &&
          (newChat ? (
            <IconButton
              tip="Back to the chat"
              tipDown
              aria-label="Back to the chat"
              onClick={() => setComposing(false)}
            >
              <Icon name="close" />
            </IconButton>
          ) : (
            <IconButton
              tip="New planning session"
              tipDown
              aria-label="New planning session"
              onClick={() => setComposing(true)}
            >
              <Icon name="plus" />
            </IconButton>
          ))}
      </div>

      {chats.error && (
        <div className="rm-thread-err flex-center text-xs">
          <span className="rm-thread-err-t">{chats.error}</span>
          <button type="button" className="rm-thread-err-x" onClick={chats.clearError}>
            Dismiss
          </button>
        </div>
      )}

      {chats.loading ? (
        <div className="rm-thread-state iflex-center text-sm">
          <Loader variant="inherit" /> Loading…
        </div>
      ) : !projectId ? (
        <div className="rm-thread-state text-sm">
          This repo isn’t part of a project yet, so it has no project manager to talk to.
        </div>
      ) : newChat ? (
        <NewChatScreen
          defaultAgentId={chats.defaultAgentId}
          starting={chats.starting}
          onStart={(pick, firstMessage) => void start(pick, firstMessage)}
          onCancel={selected ? () => setComposing(false) : undefined}
        />
      ) : selected ? (
        <ChatPane key={selected.id} agent={selected} />
      ) : null}
    </section>
  );
}
