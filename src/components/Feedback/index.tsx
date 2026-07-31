import { Icon } from "@/components/Icon";
import { Scrim } from "@/components/ui/Scrim";
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
    <>
      <Scrim onClose={onClose} zIndex={400} blur />
      <div className="fb-modal" role="dialog" aria-modal="true" aria-labelledby="fb-title">
        <div className="fb-h flex-center text-base">
          <Icon name="feedback" size={15} />
          <span id="fb-title">Send feedback</span>
          <button className="fb-close flex-center" aria-label="Close" onClick={onClose}>
            <Icon name="close" size={14} />
          </button>
        </div>
        <FeedbackForm state={state} onClose={onClose} />
      </div>
    </>
  );
}
