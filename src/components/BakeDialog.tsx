import { useEffect, useRef, useState } from "react";
import { api, onBakeProgress, type BakeProgress, type BakeStage } from "../api";

const STAGE_LABEL: Record<BakeStage, string> = {
  cloning: "Downloading base image",
  booting: "Booting VM",
  waiting_for_ssh: "Waiting for SSH",
  installing: "Installing dependencies",
  finalizing: "Finalizing",
  done: "Done",
  error: "Error",
};

const STAGE_ORDER: BakeStage[] = [
  "cloning",
  "booting",
  "waiting_for_ssh",
  "installing",
  "finalizing",
  "done",
];

const MAX_TAIL_LINES = 200;

/**
 * Drives the in-app base image build. Subscribes to `bake:progress` events
 * and renders a step-by-step status with a live tail of install output.
 *
 * The dialog deliberately cannot be closed while a bake is in flight — the
 * underlying SSH session continues regardless, but giving the user an
 * abort/close shortcut here would create the impression that the VM was
 * cleaned up when it wasn't. Better to keep the modal honest until the
 * Rust side reports `done` or `error`.
 */
export function BakeDialog({
  imageName,
  onClose,
  onSuccess,
}: {
  imageName: string;
  onClose: () => void;
  onSuccess: () => void;
}) {
  const [progress, setProgress] = useState<BakeProgress | null>(null);
  const [tail, setTail] = useState<string[]>([]);
  const [finalError, setFinalError] = useState<string | null>(null);
  const startedRef = useRef(false);
  const tailRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let unlisten: (() => void) | null = null;

    onBakeProgress((p) => {
      setProgress(p);
      if (p.tail) {
        setTail((prev) => {
          const next = [...prev, p.tail!];
          return next.length > MAX_TAIL_LINES
            ? next.slice(next.length - MAX_TAIL_LINES)
            : next;
        });
      }
      if (p.stage === "error") setFinalError(p.message);
    }).then((fn) => {
      unlisten = fn;
    });

    if (!startedRef.current) {
      startedRef.current = true;
      api.bakeBaseImage(imageName).catch((e) => {
        setFinalError(String(e));
      });
    }

    return () => {
      unlisten?.();
    };
  }, [imageName]);

  useEffect(() => {
    // Auto-scroll the tail to the bottom as new lines arrive.
    const el = tailRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [tail]);

  const stage = progress?.stage ?? "cloning";
  const done = stage === "done";
  const errored = stage === "error" || finalError !== null;
  const inFlight = !done && !errored;

  const currentIdx = STAGE_ORDER.indexOf(stage);

  return (
    <>
      <div className="backdrop" role="presentation" />
      <div className="modal modal-wide" role="dialog" aria-label="Build base image">
        <h2>
          Building base image <code>{imageName}</code>
        </h2>
        <ol className="stages">
          {STAGE_ORDER.slice(0, -1).map((s, i) => {
            const completed = !errored && (done || i < currentIdx);
            const active = !errored && !done && i === currentIdx;
            return (
              <li
                key={s}
                className={`stage ${completed ? "completed" : ""} ${active ? "active" : ""}`}
              >
                <span className="stage-dot">
                  {completed ? "✓" : active ? "●" : ""}
                </span>
                <span className="stage-label">{STAGE_LABEL[s]}</span>
              </li>
            );
          })}
        </ol>

        {progress && (
          <div className={`stage-message ${errored ? "stage-message-error" : ""}`}>
            {progress.message}
          </div>
        )}

        <div className="bake-tail" ref={tailRef}>
          {tail.length === 0 && (
            <div className="bake-tail-empty">
              Output from inside the VM will appear here.
            </div>
          )}
          {tail.map((line, i) => (
            <div key={i} className="bake-tail-line">
              {line}
            </div>
          ))}
        </div>

        <div className="actions">
          {inFlight && (
            <span className="hint">
              This takes 5–10 minutes the first time. Safe to leave running.
            </span>
          )}
          {done && (
            <button className="primary" onClick={onSuccess}>
              Use this image
            </button>
          )}
          {errored && (
            <>
              <button onClick={onClose}>Close</button>
              <button
                className="primary"
                onClick={() => {
                  setProgress(null);
                  setTail([]);
                  setFinalError(null);
                  startedRef.current = false;
                  // Force a re-mount by toggling — easier: just call again.
                  startedRef.current = true;
                  api
                    .bakeBaseImage(imageName)
                    .catch((e) => setFinalError(String(e)));
                }}
              >
                Retry
              </button>
            </>
          )}
        </div>
      </div>
    </>
  );
}
