import { useEffect, useRef, useState } from "react";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { TextInput } from "@/components/ui/TextInput";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import type { useProjectSettings } from "@/util/useProjectSettings";
import { AutonomyPopover } from "../AutonomyPopover";

/** The board's header — the PM thread's switch, the autonomy dial and "+" —
 *  and, under it on a writable board, the capture field: type a line, press
 *  Enter, and it lands in the Inbox for the PM to triage. */
export function BoardHeader({
  readOnly,
  projectSettings,
  onNew,
  onCapture,
}: {
  readOnly: boolean;
  projectSettings: ReturnType<typeof useProjectSettings>;
  onNew: () => void;
  /** Capture `text` into the inbox. Rejects on failure. */
  onCapture: (text: string) => Promise<unknown>;
}) {
  const threadOpen = useAppStore((s) => s.roadmapThreadOpen);
  const toggleThread = useAppStore((s) => s.toggleRoadmapThread);
  // A host from before the inbox drives the board but cannot land a capture:
  // the field hides rather than failing on Enter, as the phone's does.
  const captureGate = useGate("roadmapCapture");

  return (
    <>
      <div className="rm-board-h flex-center">
        <IconButton
          tip="Plan with PM"
          active={threadOpen}
          aria-pressed={threadOpen}
          onClick={toggleThread}
        >
          <Icon name="notebookPen" />
        </IconButton>
        <span className="grow" />
        {/* No shipped count here: the page header already carries it, and two
            copies of the same number a centimetre apart is clutter, not
            reinforcement. */}
        {!readOnly && (
          <>
            <AutonomyPopover {...projectSettings} />
            <IconButton aria-label="Add roadmap item" tip="Add an item" onClick={onNew}>
              <Icon name="plus" />
            </IconButton>
          </>
        )}
      </div>
      {!readOnly && !captureGate && <CaptureField onCapture={onCapture} />}
    </>
  );
}

function CaptureField({ onCapture }: { onCapture: (text: string) => Promise<unknown> }) {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const form = useRef<HTMLFormElement>(null);

  // The palette's "Add to roadmap…" opened this page to type into this field.
  const requested = useAppStore((s) => s.roadmapCaptureRequested);
  const clearRequest = useAppStore((s) => s.clearRoadmapCapture);
  useEffect(() => {
    if (!requested) return;
    form.current?.querySelector("input")?.focus();
    clearRequest();
  }, [requested, clearRequest]);

  // Cleared on Enter so a second Enter can't add the same line twice; a
  // failure puts the text back.
  async function submit() {
    const title = text.trim();
    if (!title) return;
    setText("");
    try {
      await onCapture(title);
      setError(null);
    } catch (e) {
      setText(title);
      setError(String(e));
    }
  }

  return (
    <form
      ref={form}
      className="rm-capture flex-center"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <TextInput
        className="grow"
        placeholder="Capture an idea or ticket…"
        aria-label="Capture an idea or ticket"
        value={text}
        invalid={!!error}
        title={error ?? undefined}
        onChange={(e) => setText(e.target.value)}
      />
    </form>
  );
}
