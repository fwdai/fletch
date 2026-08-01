import { useState } from "react";
import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { Scrim } from "@/components/ui/Scrim";
import { firstLine, formatAge } from "@/util/format";

/** A chat's title: the first thing the user said, which is what they'll recall
 *  it by. A chat that has never been spoken to has no title yet. */
export function chatTitle(chat: AgentRecord): string {
  const task = chat.task.trim();
  return task ? firstLine(task, 44) : "New chat";
}

/** The thread's chat switcher: which conversation you're in, every other one
 *  this project has, and the two lifecycle actions.
 *
 *  Multiple chats are the intended way to use this tab — a fresh one whenever
 *  the subject changes, rather than one thread that grows until its context is
 *  worthless — so switching and starting are both one click from the header. */
export function ChatPicker({
  chats,
  selected,
  onSelect,
  onNew,
  onDelete,
}: {
  chats: AgentRecord[];
  selected: AgentRecord;
  onSelect: (id: string) => void;
  onNew: () => void;
  onDelete: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [confirming, setConfirming] = useState<string | null>(null);
  const now = Date.now();

  const close = () => {
    setOpen(false);
    setConfirming(null);
  };

  return (
    <div className="rm-picker">
      <button
        type="button"
        className="rm-picker-btn flex-center text-sm"
        onClick={() => setOpen((v) => !v)}
        aria-label="Switch chat"
      >
        <span className="rm-picker-t truncate">{chatTitle(selected)}</span>
        <span className="rm-picker-n mono text-xs">
          {chats.length > 1 ? `${chats.length} chats` : "1 chat"}
        </span>
        <Icon name="chevD" size={9} />
      </button>

      {open && (
        <>
          <Scrim onClose={close} />
          <div className="rm-picker-menu">
            <div className="rm-picker-list">
              {chats.map((c) => (
                <div
                  key={c.id}
                  className={`rm-picker-row flex-center ${c.id === selected.id ? "active" : ""}`}
                >
                  <button
                    type="button"
                    className="rm-picker-pick text-sm"
                    onClick={() => {
                      onSelect(c.id);
                      close();
                    }}
                  >
                    <span className="rm-picker-row-t truncate">{chatTitle(c)}</span>
                    <span className="rm-picker-row-m mono text-xs">
                      {c.name} · {formatAge(c.created_at, now)}
                    </span>
                  </button>
                  {confirming === c.id ? (
                    <button
                      type="button"
                      className="rm-picker-del text-xs"
                      onClick={() => {
                        onDelete(c.id);
                        close();
                      }}
                    >
                      Delete?
                    </button>
                  ) : (
                    <IconButton
                      size="xs"
                      danger
                      tip="Delete this chat"
                      aria-label="Delete this chat"
                      onClick={() => setConfirming(c.id)}
                    >
                      <Icon name="trash" size={11} />
                    </IconButton>
                  )}
                </div>
              ))}
            </div>
            <button
              type="button"
              className="rm-picker-new flex-center text-sm"
              onClick={() => {
                onNew();
                close();
              }}
            >
              <Icon name="plus" size={11} /> New chat
            </button>
          </div>
        </>
      )}
    </div>
  );
}
