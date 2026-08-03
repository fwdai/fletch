import { type KeyboardEvent, useMemo, useRef, useState } from "react";
import { Icon } from "@/components/Icon";
import { DEFAULT_PROVIDER_ID } from "@/data/providers";
import { Composer } from "../Composer";
import type { ChatAgentPick } from "../usePmChats";
import { AgentCard } from "./AgentCard";
import { type AgentOption, useAgentOptions } from "./options";

/** Starting a planning session: pick who you want to think with, and say the
 *  first thing.
 *
 *  A screen rather than a menu. The default is nearly always right, so hiding
 *  the choice behind a popover — and the model behind a second menu inside that
 *  popover — charged two clicks for a decision most people don't make. Here the
 *  recommended agent is the only one on screen until you ask for the others, the
 *  model expands inside its own card, and the first message is part of the same
 *  act: ⏎ spawns the session and sends the prompt, instead of dropping the user
 *  into an empty thread to start typing over again.
 *
 *  It renders in the thread column's body, leaving the board beside it live —
 *  a modal would blank half the tab for a two-second decision. The composer
 *  stays exactly where the real chat's composer is, so starting a session reads
 *  as the conversation beginning rather than as a form being submitted. */
export function NewChatScreen({
  defaultAgentId,
  starting,
  onStart,
  onCancel,
}: {
  /** The Project Manager preset, when it has resolved. */
  defaultAgentId: string | undefined;
  starting: boolean;
  /** Spawn the chat, sending `firstMessage` as its opening turn when there is
   *  one. */
  onStart: (pick: ChatAgentPick, firstMessage?: string) => void;
  /** Back to the chat that was open, or undefined when there is none to go back
   *  to — the project's first chat is started from this same screen. */
  onCancel?: () => void;
}) {
  const { custom, providers, all } = useAgentOptions(defaultAgentId);
  const [picked, setPicked] = useState<ChatAgentPick | null>(null);
  const [modelsOpen, setModelsOpen] = useState(false);
  const cards = useRef<HTMLDivElement>(null);

  // The default follows `defaultAgentId` until the user touches the picker —
  // the preset may still be seeding on first render, and the screen must not
  // lock in a bare provider just because it drew a frame early.
  const fallback = useMemo<ChatAgentPick>(
    () => custom[0]?.pick ?? { provider: DEFAULT_PROVIDER_ID },
    [custom],
  );
  const pick = picked ?? fallback;
  const selectedKey = pick.customAgentId ?? pick.provider;
  // The default can itself be unavailable — an uninstalled Claude leaves the PM
  // preset unspawnable — so the screen refuses to start rather than letting the
  // spawn fail into the error strip.
  const blocked = all.find((o) => o.key === selectedKey)?.disabled ?? null;

  // The bare coding agents are the fallback choice, so they start folded away and
  // the screen opens on the one agent it recommends. Same shape as `pick`: null
  // means "nobody has said", which resolves to *whether the selection is down
  // there* — so a project with no PM preset (still seeding, or its provider
  // disabled) never opens with its own selection hidden.
  const [providersOpen, setProvidersOpen] = useState<boolean | null>(null);
  const providersExpanded = providersOpen ?? providers.some((o) => o.key === selectedKey);

  const choose = (option: AgentOption) => {
    setPicked(option.pick);
    // The expanded model list belongs to the card that was selected; carrying
    // it over to a different agent would show another provider's catalog.
    setModelsOpen(false);
    // Reached by keyboard, the pick can land in the folded group — unfold it
    // rather than moving the selection somewhere the user can't see.
    if (option.provider) setProvidersOpen(true);
  };

  /** Move selection and focus within the card group — the arrow-key half of the
   *  radiogroup contract, since `role="radio"` gets none of it for free. */
  const step = (delta: number) => {
    const usable = all.filter((o) => !o.disabled);
    if (usable.length === 0) return;
    const at = usable.findIndex((o) => o.key === selectedKey);
    const next = usable[(at + delta + usable.length) % usable.length];
    choose(next);
    // The card group re-renders with the new roving tabstop; move focus onto it.
    requestAnimationFrame(() => {
      cards.current?.querySelector<HTMLElement>('[role="radio"][tabindex="0"]')?.focus();
    });
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape" && onCancel) {
      e.preventDefault();
      onCancel();
      return;
    }
    // ⌘1…⌘9 jumps straight to an agent from anywhere on the screen, including
    // mid-sentence in the composer.
    if ((e.metaKey || e.ctrlKey) && /^[1-9]$/.test(e.key)) {
      const option = all[Number(e.key) - 1];
      if (option && !option.disabled) {
        e.preventDefault();
        choose(option);
      }
      return;
    }
    // Arrows only steer the cards while focus is on one; inside the composer
    // they belong to the caret.
    const onCard = (e.target as HTMLElement).getAttribute("role") === "radio";
    if (onCard && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      step(e.key === "ArrowDown" ? 1 : -1);
    }
  };

  const cardsFor = (options: AgentOption[]) =>
    options.map((o) => (
      <AgentCard
        key={o.key}
        option={o}
        selected={o.key === selectedKey}
        model={o.key === selectedKey ? pick.model : undefined}
        modelsOpen={modelsOpen}
        onSelect={() => choose(o)}
        onToggleModels={() => setModelsOpen((v) => !v)}
        onPickModel={(model) => {
          setPicked({ ...o.pick, model });
          setModelsOpen(false);
        }}
      />
    ));

  return (
    /* The keydown handler is a shortcut scope for the screen's own focusable
       children (the cards and the composer), not a control in itself. */
    <div className="rm-nc" onKeyDown={onKeyDown}>
      <div className="rm-nc-scroll">
        <div className="rm-blank rm-nc-blank">
          {/* Planning, not magic — and deliberately not the board's `map`, which
              badges the empty board a splitter away. You draft here; it lands
              there. */}
          <span className="rm-blank-badge iflex-center">
            <Icon name="notebookPen" size={18} />
          </span>
          <h3 className="rm-blank-h text-base">Start a planning session</h3>
          <p className="rm-blank-b text-sm">
            A fresh thread, with its own workspace and context. It reads the repo before it answers,
            never edits code, and nothing reaches the board until you accept it.
          </p>
        </div>

        <div
          className="rm-nc-cards"
          ref={cards}
          role="radiogroup"
          aria-label="Agent for this session"
        >
          {custom.length > 0 && (
            <>
              <div className="rm-nc-sect flex-center text-xs">
                <span>Project manager</span>
                <span className="rm-nc-sect-line" />
              </div>
              {cardsFor(custom)}
            </>
          )}

          {providers.length > 0 &&
            (custom.length > 0 ? (
              // Folded away by default: these are the fallback, and the screen
              // should open on its recommendation, not on a wall of seven cards.
              <>
                <button
                  type="button"
                  className="rm-nc-sect rm-nc-sect-btn flex-center text-xs"
                  aria-expanded={providersExpanded}
                  onClick={() => setProvidersOpen(!providersExpanded)}
                >
                  <span>Default agents</span>
                  <span className="rm-nc-sect-line" />
                  <span className="rm-nc-sect-n">{providers.length}</span>
                  <Icon name={providersExpanded ? "chevU" : "chevD"} size={9} />
                </button>
                {providersExpanded && cardsFor(providers)}
              </>
            ) : (
              // No PM preset to recommend, so these are the whole choice and
              // there is nothing to fold them behind.
              <>
                <div className="rm-nc-sect flex-center text-xs">
                  <span>Coding agents</span>
                  <span className="rm-nc-sect-line" />
                </div>
                {cardsFor(providers)}
              </>
            ))}
        </div>
      </div>

      <Composer
        autoFocus
        disabled={starting || !!blocked}
        placeholder={
          blocked
            ? blocked
            : starting
              ? "Starting the session…"
              : "Describe an outcome, a complaint, a half-formed idea…"
        }
        onSend={(text) => onStart(pick, text)}
        hint={
          <div className="rm-nc-foot iflex-center text-xs">
            <span>⏎ to start · ⇧⏎ for a new line</span>
            <span className="rm-nc-dot">·</span>
            <button
              type="button"
              className="rm-nc-empty"
              disabled={starting || !!blocked}
              onClick={() => onStart(pick)}
            >
              start an empty session
            </button>
          </div>
        }
      />
    </div>
  );
}
