import { useCallback, useRef, useState } from "react";
import { removeHost } from "@/remote/hosts";
import type { NearbyHost } from "@/remote/nearby";
import type { AddHost } from "../usePairedHosts";
import { afterFailure, type NearbyStep, pairTargetFor } from "./nearbyPairing";

export interface NearbyPairing {
  step: NearbyStep;
  /** Why the last attempt failed. Cleared by the next one. */
  error: string | null;
  /** Ask `host` to pair, to be accepted on its screen. */
  pick: (host: NearbyHost) => void;
  /** Pair with the code the host's screen shows. */
  submitCode: (code: string) => void;
  /** Give up on the attempt in flight, if any, and go back to the list. */
  cancel: () => void;
}

/** Pairing with a host picked from the nearby list: ask, and show the six
 *  digits while its owner decides; a host with nobody at its screen asks for
 *  its code instead (`afterFailure`). */
export function useNearbyPairing(add: AddHost): NearbyPairing {
  const [step, setStep] = useState<NearbyStep>({ kind: "idle" });
  const [error, setError] = useState<string | null>(null);
  // Which attempt owns the flow: a cancelled one finishing late must not move
  // it, nor a code it hears.
  const attempt = useRef(0);
  // The host an attempt is dialling, so cancelling can hang up on it.
  const dialling = useRef<string | null>(null);

  const run = useCallback(
    async (host: NearbyHost, code?: string) => {
      const id = ++attempt.current;
      const target = pairTargetFor(host, code);
      if (!target) {
        setStep({ kind: "idle" });
        setError(`${host.name} announced a key that can't be paired with.`);
        return;
      }
      setError(null);
      dialling.current = host.hostKey;
      try {
        await add(target, (digits) => {
          if (attempt.current === id) setStep({ kind: "asking", host, code: digits });
        });
        if (attempt.current === id) setStep({ kind: "idle" });
      } catch (e) {
        if (attempt.current !== id) return;
        const next = afterFailure(
          host,
          code !== undefined,
          e instanceof Error ? e.message : String(e),
        );
        setStep(next.step);
        setError(next.error);
      } finally {
        if (attempt.current === id) dialling.current = null;
      }
    },
    [add],
  );

  const pick = useCallback(
    (host: NearbyHost) => {
      setStep({ kind: "asking", host, code: null });
      void run(host);
    },
    [run],
  );

  const submitCode = useCallback(
    (code: string) => {
      if (step.kind !== "code" || step.sending) return;
      setStep({ ...step, sending: true });
      void run(step.host, code);
    },
    [run, step],
  );

  const cancel = useCallback(() => {
    attempt.current += 1;
    // The attempt is not saved yet, so dropping its connection is all there
    // is to undo; the host takes its prompt down when the socket closes.
    const hostKey = dialling.current;
    dialling.current = null;
    if (hostKey) void removeHost(hostKey).catch(() => {});
    setStep({ kind: "idle" });
    setError(null);
  }, []);

  return { step, error, pick, submitCode, cancel };
}
