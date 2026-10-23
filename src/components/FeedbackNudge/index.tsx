// The one-time "we'd love your feedback" prompt. Mounted once at the app root;
// renders nothing until its schedule (`nudge.ts`) says it is the third distinct
// day the app has been opened, or Settings › Developer previews it. Three ways
// out: book a short call, write an email, or dismiss — each closes it for good.

import { open as openExternal } from "@tauri-apps/plugin-shell";
import { useEffect, useRef } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { Modal, ModalBody } from "@/components/ui/Modal";
import { getSetting, setSetting } from "@/storage/settings";
import { useAppStore } from "@/store";
import { track } from "@/util/track";
import { createScheduler, FEEDBACK_NUDGE_KEY, localDay } from "./nudge";
import "./FeedbackNudge.css";

/** Where "Book a call" lands: a 15-minute slot with Alex. */
const CALL_URL = "https://calendly.com/alchaplinsky/call";

/** Where "Write an email" lands: Alex directly, not the shared feedback inbox
 *  the Send-feedback modal falls back to — this is a personal invitation. */
const CONTACT_EMAIL = "alex@fletch.sh";

const EMAIL_URL = `mailto:${CONTACT_EMAIL}?subject=${encodeURIComponent("Fletch feedback")}`;

export function FeedbackNudge() {
  useNudgeSchedule();
  const open = useAppStore((s) => s.feedbackNudgeOpen);
  const close = useAppStore((s) => s.closeFeedbackNudge);
  if (!open) return null;
  return <FeedbackNudgeModal onClose={close} />;
}

/** Feed the schedule (`createScheduler`) the two store flags it keys on. It
 *  waits for settings hydration before counting today — the slice default says
 *  onboarding is closed until hydration says otherwise — and for onboarding to
 *  close before raising the prompt, so a first-run user's day on the welcome
 *  screen counts as a day of use without a second dialog landing on the tour. */
function useNudgeSchedule() {
  const hydrated = useAppStore((s) => s.settingsHydrated);
  const onboardingOpen = useAppStore((s) => s.onboardingOpen);
  // One scheduler for the life of the mount; its collaborators are module
  // functions, so nothing here goes stale.
  const tick = useRef(
    createScheduler({
      load: () => getSetting(FEEDBACK_NUDGE_KEY),
      save: (n) => setSetting(FEEDBACK_NUDGE_KEY, n),
      open: () => useAppStore.getState().openFeedbackNudge(),
      today: localDay,
    }),
  ).current;
  useEffect(() => {
    void tick({ hydrated, onboardingOpen });
  }, [tick, hydrated, onboardingOpen]);
}

function FeedbackNudgeModal({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    track("feedback_nudge_shown", {});
  }, []);

  const answer = (action: "call" | "email" | "dismiss", url?: string) => {
    track("feedback_nudge_answered", { action });
    if (url) void openExternal(url).catch(() => {});
    onClose();
  };

  return (
    <Modal
      icon="megaphone"
      title="A note from the Fletch team"
      onClose={() => answer("dismiss")}
      size="lg"
    >
      <ModalBody center className="fn-body">
        <div className="fn-mark" aria-hidden="true">
          <Icon name="megaphone" size={20} />
        </div>
        <h2 className="fn-title text-xl">How's Fletch working for you?</h2>
        <p className="fn-lede text-sm">
          You've been using Fletch for a few days now, and we'd love to hear how it's going. What
          works, what gets in the way, what you wish it did. Fifteen minutes with Alex, who builds
          Fletch, is the best way to tell us. An email is great too.
        </p>
        <div className="fn-actions">
          <Button variant="primary" size="lg" onClick={() => answer("call", CALL_URL)}>
            <Icon name="calendar" size={14} />
            Book a 15-min call
          </Button>
          <Button variant="outline" size="lg" onClick={() => answer("email", EMAIL_URL)}>
            <Icon name="mail" size={14} />
            Write an email
          </Button>
        </div>
        <Button variant="ghost" size="sm" className="fn-skip" onClick={() => answer("dismiss")}>
          No thanks
        </Button>
      </ModalBody>
    </Modal>
  );
}
