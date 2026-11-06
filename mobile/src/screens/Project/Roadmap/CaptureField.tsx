import { Icon } from "@desktop/components/Icon";
import { useState } from "react";
import { PromptField } from "../../../components/PromptField";
import { Notice } from "../../../components/ui/Notice";
import { useStore } from "../../../store";

/** The board's capture field, on the phone: type or dictate a note and it lands
 *  in the project's Inbox as a `captured` row for the PM to triage — the same
 *  `roadmap_capture` the Mac's field sends. A dictated note is a draft like a
 *  typed one; nothing goes out until Capture is tapped. */
export function CaptureField({ projectId }: { projectId: string }) {
  const capture = useStore((s) => s.captureRoadmapItem);
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A live mic holds the button: the words are still on their way into the draft.
  const [dictating, setDictating] = useState(false);

  const note = text.trim();
  const submit = () => {
    if (!note || dictating || sending) return;
    setSending(true);
    setError(null);
    capture(projectId, note)
      // Only a note the host took is spent; a refused one stays, with the
      // reason, so the button can simply be pressed again.
      .then(
        () => setText(""),
        (e) => setError(e instanceof Error ? e.message : String(e)),
      )
      .finally(() => setSending(false));
  };

  return (
    <>
      <PromptField
        value={text}
        onChange={setText}
        onDictating={setDictating}
        placeholder="Capture an idea or ticket…"
      />
      {error && (
        <Notice tone="error" className="na-err" onDismiss={() => setError(null)}>
          {error}
        </Notice>
      )}
      <button
        type="button"
        className="btn primary sm block"
        style={{ marginTop: 10 }}
        disabled={!note || dictating || sending}
        onClick={submit}
      >
        <Icon name="inbox" size={15} />
        {sending ? "Capturing…" : "Capture"}
      </button>
    </>
  );
}
