import { useCallback, useEffect, useState } from "react";
import type { PairRequest } from "@/api";
import { useEngine } from "@/api/engine";

/** The device waiting for this Mac to accept it, if one is, and the answer.
 *  Driven by the host's events, and seeded once from `remote_status` so a
 *  window opened mid-request still shows it. */
export function usePairRequest(): {
  request: PairRequest | null;
  answer: (accept: boolean) => void;
} {
  const { api, events } = useEngine();
  const [request, setRequest] = useState<PairRequest | null>(null);

  useEffect(() => {
    let live = true;
    const offs = [
      events.onPairRequest((r) => setRequest(r)),
      events.onPairRequestEnded(({ id }) => setRequest((r) => (r?.id === id ? null : r))),
    ];
    void api
      .remoteStatus()
      .then((s) => {
        if (live && s.pairRequest) setRequest(s.pairRequest);
      })
      .catch(() => {});
    return () => {
      live = false;
      for (const off of offs) void off.then((f) => f());
    };
  }, [api, events]);

  const answer = useCallback(
    (accept: boolean) => {
      if (!request) return;
      setRequest(null);
      void api.remoteAnswerPairRequest(request.id, accept).catch(() => {});
    },
    [request, api],
  );

  return { request, answer };
}
