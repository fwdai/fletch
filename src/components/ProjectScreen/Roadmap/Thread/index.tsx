import { useState } from "react";
import { AgentIdentityChip } from "@/components/AgentIdentityChip";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { Loader } from "@/components/ui/Loader";
import { Scrim } from "@/components/ui/Scrim";
import type { RoadmapState } from "../useRoadmap";
import { ChatPane } from "./ChatPane";
import { ChatPicker } from "./ChatPicker";
import { NewChatForm } from "./NewChatForm";
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
  /** The new-chat popover, opened from the header while a chat is already up.
   *  With no chats yet the same form is the body's empty state, so there is
   *  nothing to pop over. */
  const [composing, setComposing] = useState(false);

  const start = (pick: ChatAgentPick) => {
    setComposing(false);
    void chats.startChat(pick);
  };

  return (
    <section className="rm-thread">
      <div className="rm-thread-head flex-center">
        {selected ? (
          <AgentIdentityChip agent={selected} size={20} />
        ) : (
          <span className="rm-pm-badge iflex-center">
            <Icon name="sparkle" size={12} />
          </span>
        )}

        {selected ? (
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

        {selected && (
          <div className="rm-newchat-anchor">
            <IconButton
              tip="New chat"
              tipDown
              aria-label="New chat"
              active={composing}
              onClick={() => setComposing((v) => !v)}
            >
              <Icon name="plus" />
            </IconButton>
            {composing && (
              <>
                <Scrim onClose={() => setComposing(false)} />
                <div className="rm-newchat-pop">
                  <div className="rm-newchat-h text-xs">
                    A fresh thread, with its own workspace and context.
                  </div>
                  <NewChatForm
                    defaultAgentId={chats.defaultAgentId}
                    starting={chats.starting}
                    onStart={start}
                  />
                </div>
              </>
            )}
          </div>
        )}
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
      ) : selected ? (
        <ChatPane key={selected.id} agent={selected} />
      ) : (
        <div className="rm-thread-scroll">
          <div className="rm-blank rm-thread-blank">
            <span className="rm-blank-badge iflex-center">
              <Icon name="sparkle" size={18} />
            </span>
            <h3 className="rm-blank-h text-base">Talk to your project manager</h3>
            <p className="rm-blank-b text-sm">
              It reads the repo first — what exists, what depends on what, where the seams are —
              then proposes roadmap items with the code to back them up. It never edits code, and
              nothing reaches the board until you accept it.
            </p>
            <NewChatForm
              defaultAgentId={chats.defaultAgentId}
              starting={chats.starting}
              onStart={start}
            />
          </div>
        </div>
      )}
    </section>
  );
}
