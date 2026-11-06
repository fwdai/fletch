import { useState } from "react";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { TextInput } from "@/components/ui/TextInput";
import { useAppStore } from "@/store";
import type { useProjectSettings } from "@/util/useProjectSettings";
import { AutonomyPopover } from "../AutonomyPopover";

/** The board's header — the PM thread's switch, the autonomy dial and "+" —
 *  and, under it on a writable board, the capture field: type a line, press
 *  Enter, and it is on the board under Later. */
export function BoardHeader({
  readOnly,
  projectSettings,
  onNew,
  onCapture,
}: {
  readOnly: boolean;
  projectSettings: ReturnType<typeof useProjectSettings>;
  onNew: () => void;
  /** Add an item titled `title`. Rejects on failure. */
  onCapture: (title: string) => Promise<unknown>;
}) {
  const threadOpen = useAppStore((s) => s.roadmapThreadOpen);
  const toggleThread = useAppStore((s) => s.toggleRoadmapThread);

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
      {!readOnly && <CaptureField onCapture={onCapture} />}
    </>
  );
}

function CaptureField({ onCapture }: { onCapture: (title: string) => Promise<unknown> }) {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);

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
