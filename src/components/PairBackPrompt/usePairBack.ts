import { useCallback, useState } from "react";
import { offerPairBack } from "@/remote/hosts";
import { useAppStore } from "@/store";
import type { PairBackOffer } from "@/store/app";

/** The pair-back question on screen, and its two answers. Allow mints this
 *  Mac's pairing link and hands it to the host; a failure stays on the
 *  question, to read and then retry or dismiss. */
export function usePairBack(): {
  offer: PairBackOffer | null;
  sending: boolean;
  error: string | null;
  allow: () => void;
  dismiss: () => void;
} {
  const offer = useAppStore((s) => s.pairBackOffer);
  const setOffer = useAppStore((s) => s.setPairBackOffer);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const dismiss = useCallback(() => {
    setError(null);
    setOffer(null);
  }, [setOffer]);

  const allow = useCallback(() => {
    if (!offer || sending) return;
    setSending(true);
    setError(null);
    offerPairBack(offer.hostKey)
      .then(
        () => setOffer(null),
        (e: unknown) => setError(e instanceof Error ? e.message : String(e)),
      )
      .finally(() => setSending(false));
  }, [offer, sending, setOffer]);

  return { offer, sending, error, allow, dismiss };
}
