import type { MouseEvent, ReactNode } from "react";
import { Icon } from "@/components/Icon";
import { Loader } from "@/components/ui/Loader";
import { ContourTrace } from "./ContourTrace";
import { LevelBars } from "./LevelBars";
import { formatClock, type PrimaryState } from "./primaryState";
import { useElapsed } from "./useElapsed";

export { type DictationPhase, type PrimaryState, primaryState } from "./primaryState";

/** What the user has to do outside the app. The backend answers a blocked start
 *  with the same guidance, so the tooltip reads identically whether we knew up
 *  front or found out on the attempt. */
export const SETTINGS_HINT = "Microphone is off — allow it in System Settings";

interface Props {
  state: PrimaryState;
  /** False where no engine exists (Linux, Windows, a Mac below 26 without the
   *  local engine): the mic is never offered, and the empty pill degrades to a
   *  plain send arrow — the control the composer had before. */
  dictationAvailable: boolean;
  /** The local engine ends the session itself once the user pauses, and the
   *  tooltip has to say so — a mic that stops on its own otherwise reads as a
   *  bug. */
  autoStops: boolean;
  /** Recent microphone levels, oldest first, each 0–1. */
  levels: number[];
  /** When the mic opened (epoch ms), for the clock; null when it isn't open. */
  startedAt: number | null;
  /** Why sending is blocked right now, if it is (a provider the sandbox engine
   *  can't run). Shown as the tooltip in `draft`; the arrow is disabled. */
  sendBlocked?: string;
  /** The last failure's reason, shown as the error pill's tooltip. */
  error: string | null;
  /** The composer can't take input (agent not ready, transcript loading). The
   *  mic and send are inert; stopping a run stays possible. */
  disabled?: boolean;
  onMic: () => void;
  onSend: () => void;
  onStopDictation: () => void;
  onStopRun: () => void;
  onRetry: () => void;
  onUnavailable: () => void;
}

/** The composer's primary control: the mic when the field is empty, the send
 *  arrow once there is a draft (the mic steps aside to its left as a neutral
 *  secondary), the live waveform + clock + stop while listening, a label while
 *  transcribing, the stop square with a running trace while the agent works.
 *  The pill's right edge is pinned; everything grows or slides leftward.
 *
 *  Presentational — the state is derived by the composer (see `primaryState`)
 *  and every action is a callback. */
export function PrimaryControl({
  state,
  dictationAvailable,
  autoStops,
  levels,
  startedAt,
  sendBlocked,
  error,
  disabled = false,
  onMic,
  onSend,
  onStopDictation,
  onStopRun,
  onRetry,
  onUnavailable,
}: Props) {
  const is = (s: PrimaryState) => state === s;
  const elapsed = useElapsed(is("listening") ? startedAt : null);

  // In `draft` the mic steps aside and the primary becomes send. Without an
  // engine the mic never shows, and `empty` already holds a (disabled) arrow so
  // the slot never reads as blank.
  const split = is("draft");
  const micHome = dictationAvailable && is("empty");
  const micAside = dictationAvailable && split;
  const showSend = split || (!dictationAvailable && is("empty"));
  const sendDisabled = disabled || !!sendBlocked || !is("draft");

  const tip = tipFor(state, { dictationAvailable, autoStops, sendBlocked, error });

  // The whole pill is one target in the states that have one action.
  function onPillClick() {
    if (is("listening")) onStopDictation();
    else if (is("running")) onStopRun();
    else if (is("error") && !disabled) onRetry();
    else if (is("unavailable")) onUnavailable();
  }

  const pillClass = [
    "pc",
    `s-${state}`,
    split ? "split" : "",
    dictationAvailable ? "" : "no-mic",
    disabled ? "is-disabled" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <span className="pc-wrap tip" data-tip={tip}>
      <button
        type="button"
        className={`pc-sec${micAside ? " aside" : ""}${micHome || micAside ? "" : " gone"}`}
        aria-label={micAside ? "Dictate more" : "Dictate"}
        tabIndex={micHome || micAside ? 0 : -1}
        disabled={disabled || !(micHome || micAside)}
        onClick={onMic}
      >
        <Icon name="mic" size={14} />
      </button>
      {/* Every action has a real <button> inside; the group's click is the same
       *  target widened to the whole pill in the states that have one action. */}
      <div
        className={pillClass}
        role="group"
        aria-label="Composer primary control"
        onClick={onPillClick}
      >
        <ContourTrace active={is("running")} />
        <Seg on={is("listening")} className="wave">
          <LevelBars levels={levels} />
        </Seg>
        <Seg on={is("listening")} className="timer">
          {formatClock(elapsed)}
        </Seg>
        <Seg on={is("transcribing")} className="label">
          Transcribing
          <Loader variant="accent" size="sm" className="pc-dots" aria-hidden />
        </Seg>
        <Seg on={is("error")} className="label wide">
          <Icon name="refresh" size={12} />
          Couldn't transcribe
        </Seg>
        <Seg on={is("unavailable")} className="sq unav">
          <Icon name="micOff" size={14} />
        </Seg>
        <SegButton
          on={is("listening")}
          className="sq stopd"
          label="Stop dictation"
          onClick={onStopDictation}
        >
          <span className="pc-stop-sq" />
        </SegButton>
        <SegButton on={is("running")} className="sq stopr" label="Stop agent" onClick={onStopRun}>
          <span className="pc-stop-sq" />
        </SegButton>
        <SegButton
          on={showSend}
          className="sq send"
          label="Send"
          disabled={sendDisabled}
          onClick={onSend}
        >
          <Icon name="arrowUp" size={14} strokeWidth={1.75} />
        </SegButton>
      </div>
    </span>
  );
}

/** One segment of the pill. Width animates 0 → its own width on `on`; the
 *  glyph inside scales in with it. Non-interactive: the pill's own click
 *  handles states with a single action. */
function Seg({ on, className, children }: { on: boolean; className: string; children: ReactNode }) {
  return (
    <span className={`seg ${className}${on ? " on" : ""}`}>
      <span className="seg-in">
        <span className="glyph">{children}</span>
      </span>
    </span>
  );
}

/** A segment that is its own button, so it has its own label and focus stop.
 *  Its click doesn't reach the pill's — two targets, two actions. */
function SegButton({
  on,
  className,
  label,
  disabled,
  onClick,
  children,
}: {
  on: boolean;
  className: string;
  label: string;
  disabled?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  function click(e: MouseEvent<HTMLButtonElement>) {
    e.stopPropagation();
    onClick();
  }
  return (
    <button
      type="button"
      className={`seg ${className}${on ? " on" : ""}`}
      aria-label={label}
      tabIndex={on ? 0 : -1}
      disabled={disabled || !on}
      onClick={click}
    >
      <span className="seg-in">
        <span className="glyph">{children}</span>
      </span>
    </button>
  );
}

function tipFor(
  state: PrimaryState,
  o: {
    dictationAvailable: boolean;
    autoStops: boolean;
    sendBlocked?: string;
    error: string | null;
  },
): string {
  switch (state) {
    case "empty":
      return o.dictationAvailable ? "Dictate · ⌘⇧D" : "Send · ↵";
    case "draft":
      if (o.sendBlocked) return o.sendBlocked;
      return o.dictationAvailable ? "Send · ↵  ·  Dictate more · ⌘⇧D" : "Send · ↵";
    case "listening":
      return o.autoStops
        ? "Listening… stops when you pause  ·  Esc cancels"
        : "Stop & transcribe · ⌘⇧D  ·  Esc cancels";
    case "transcribing":
      return "Transcribing…";
    case "running":
      return "Stop agent · Esc";
    case "error":
      return o.error ?? "Couldn't transcribe · click to retry";
    case "unavailable":
      return SETTINGS_HINT;
  }
}
