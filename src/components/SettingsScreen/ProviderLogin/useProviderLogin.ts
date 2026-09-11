import { open } from "@tauri-apps/plugin-shell";
import { WebLinksAddon } from "@xterm/addon-web-links";
import type { Terminal } from "@xterm/xterm";
import { useCallback, useEffect, useRef, useState } from "react";
import type { ProviderLoginExitEvent } from "@/api";
import { useXterm } from "@/util/useXterm";
import {
  attachLogin,
  closeLogin,
  getLoginExit,
  readLoginBuffer,
  registerLoginSink,
  resizeLogin,
  runLogin,
  subscribeLoginExit,
  writeLogin,
} from "./loginSessions";

/** Mount a terminal running a provider's sign-in command and keep it wired to
 *  the live PTY: replay what the flow has already printed, stream new output,
 *  forward keystrokes and resizes, and open any URL it prints in the browser
 *  (OAuth flows print one when they can't launch a browser themselves).
 *
 *  `exit` is the flow's outcome once it ends, `undefined` while it runs.
 *  `runAgain` restarts it on a cleared screen; `close` is the only path that
 *  kills the PTY — unmounting leaves it running so collapsing the row
 *  mid-login doesn't abort it. */
export function useProviderLogin(providerId: string) {
  const [exit, setExit] = useState<ProviderLoginExitEvent | undefined>(() =>
    getLoginExit(providerId),
  );
  const termRef = useRef<Terminal | null>(null);

  useEffect(() => {
    setExit(getLoginExit(providerId));
    return subscribeLoginExit(providerId, setExit);
  }, [providerId]);

  const containerRef = useXterm(
    { fontSize: 12, lineHeight: 1.2, scrollback: 5000 },
    (term) => {
      termRef.current = term;
      term.loadAddon(new WebLinksAddon((_, url) => open(url)));

      const buffered = readLoginBuffer(providerId);
      if (buffered && buffered.length > 0) term.write(buffered);

      // The PTY opens at the terminal's pre-fit size; xterm's first fit fires
      // `onResize` right after this, which corrects it (and waits for the open
      // to land — see `resizeLogin`).
      attachLogin(providerId, term.cols, term.rows);

      const onResize = term.onResize(({ cols, rows }) => {
        resizeLogin(providerId, cols, rows);
      });
      const onData = term.onData((data) => {
        writeLogin(providerId, data);
      });
      const unregister = registerLoginSink(providerId, (bytes) => term.write(bytes));

      return () => {
        unregister();
        onResize.dispose();
        onData.dispose();
        termRef.current = null;
        // NOTE: no closeProviderLogin here — a user who collapses the row
        // mid-login must not abort it. Only the Close button kills the PTY.
      };
    },
    [providerId],
  );

  const runAgain = useCallback(() => {
    const term = termRef.current;
    term?.reset();
    runLogin(providerId, term?.cols ?? 80, term?.rows ?? 24);
    term?.focus();
  }, [providerId]);

  const close = useCallback(() => closeLogin(providerId), [providerId]);

  return { containerRef, exit, runAgain, close };
}
