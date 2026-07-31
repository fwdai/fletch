import { Modal } from "@/components/ui/Modal";
import { useAppStore } from "@/store";
import { FeedbackForm } from "./FeedbackForm";
import { useFeedback } from "./useFeedback";

/** Send-feedback modal, opened from the sidebar footer's speech-bubble button.
 *  Mounted once at the app root (like `GithubConnectModal`) and renders nothing
 *  until opened, so the form state is fresh on every open. */
export function Feedback() {
  const open = useAppStore((s) => s.feedbackOpen);
  const close = useAppStore((s) => s.closeFeedback);
  if (!open) return null;
  return <FeedbackModal onClose={close} />;
}

/** Split from `Feedback` so the hook (and the form state it owns) mounts with
 *  the modal and unmounts with it — no stale draft on the next open. */
function FeedbackModal({ onClose }: { onClose: () => void }) {
  const state = useFeedback(onClose);

  return (
    <Modal icon="feedback" title="Send feedback" onClose={onClose} layer="overlay">
      <FeedbackForm state={state} onClose={onClose} />
    </Modal>
  );
}
