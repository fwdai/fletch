// State for the send-feedback modal: the form fields, the screenshot pick, and
// the submit lifecycle. Kept out of the components so the modal shell stays
// presentational and the flow is readable in one place.

import { open as openExternal } from "@tauri-apps/plugin-shell";
import { useEffect, useState } from "react";
import { api } from "@/api";
import { useAppStore } from "@/store";
import { type PickedImage, pickImageAsBase64 } from "@/util/image";

/** Where feedback lands when the in-app path can't be used. Also the address the
 *  modal names, so nobody has to guess where their message went. */
export const FEEDBACK_EMAIL = "feedback@fletch.sh";

/** Cap on the base64 screenshot, matching `MAX_SCREENSHOT_B64` in
 *  `src-tauri/src/commands/feedback.rs` — both derive from PostHog's 1 MB
 *  per-event limit, with headroom for the message and super-props. */
const MAX_SCREENSHOT_B64 = 600 * 1024;

/** How long the "Thanks" state lingers before the modal closes itself. */
const SENT_LINGER_MS = 1400;

export type FeedbackStatus = "idle" | "sending" | "sent" | "error";

export function useFeedback(onClose: () => void) {
  const accountEmail = useAppStore((s) => s.account?.email);

  const [message, setMessage] = useState("");
  // Prefilled from the local account as a starting value only — this hook mounts
  // with the modal, so clearing the field to stay anonymous sticks for the life
  // of the form and starts fresh on the next open.
  const [email, setEmail] = useState(accountEmail ?? "");
  const [shot, setShot] = useState<PickedImage | null>(null);
  const [shotError, setShotError] = useState<string | null>(null);
  const [status, setStatus] = useState<FeedbackStatus>("idle");
  const [error, setError] = useState<string | null>(null);

  // Close on our own once the thank-you has been read.
  useEffect(() => {
    if (status !== "sent") return;
    const t = setTimeout(onClose, SENT_LINGER_MS);
    return () => clearTimeout(t);
  }, [status, onClose]);

  const canSend = message.trim().length > 0 && status !== "sending" && status !== "sent";

  async function attachScreenshot() {
    setShotError(null);
    try {
      const picked = await pickImageAsBase64({
        maxBytes: MAX_SCREENSHOT_B64,
        title: "Attach a screenshot",
      });
      // `null` means the user dismissed the picker — leave any existing
      // attachment alone rather than silently dropping it.
      if (picked) setShot(picked);
    } catch (e) {
      setShotError(e instanceof Error ? e.message : String(e));
    }
  }

  function removeScreenshot() {
    setShot(null);
    setShotError(null);
  }

  async function submit() {
    if (!canSend) return;
    setStatus("sending");
    setError(null);
    try {
      await api.submitFeedback({
        message,
        contactEmail: email,
        screenshotBase64: shot?.base64 ?? null,
        source: "sidebar",
      });
      setStatus("sent");
    } catch (e) {
      setStatus("error");
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  /** Fall back to the user's mail client with everything they typed prefilled,
   *  so a failed send doesn't cost them the message. The address comes first in
   *  the URL because the shell plugin's default scope requires a word character
   *  straight after `mailto:`. Screenshots can't ride a mailto — the body says
   *  so rather than dropping one silently. */
  function emailInstead() {
    const body = shot
      ? `${message}\n\n(I had a screenshot attached — happy to send it as a reply.)`
      : message;
    const url = `mailto:${FEEDBACK_EMAIL}?subject=${encodeURIComponent(
      "Fletch feedback",
    )}&body=${encodeURIComponent(body)}`;
    void openExternal(url);
  }

  return {
    message,
    setMessage,
    email,
    setEmail,
    shot,
    shotError,
    status,
    error,
    canSend,
    attachScreenshot,
    removeScreenshot,
    submit,
    emailInstead,
  };
}

export type FeedbackState = ReturnType<typeof useFeedback>;
