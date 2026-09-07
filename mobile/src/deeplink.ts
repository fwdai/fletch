// `fletch://pair?…` arriving from a QR reader or a link on the host. Inside
// Tauri the deep-link plugin delivers it; in a browser the same URL can be
// pasted into the Pair screen, so there is nothing to register.

import { onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { ignore } from "./lib/ignore";
import { parsePairUrl } from "./remote";
import { inTauri } from "./remote/ws";
import { useStore } from "./store";

export async function registerDeepLinks(): Promise<void> {
  if (!inTauri()) return;
  try {
    await onOpenUrl((urls) => {
      for (const url of urls) {
        const target = parsePairUrl(url);
        if (target) {
          void useStore.getState().connect(target).catch(ignore);
          return;
        }
      }
    });
  } catch {
    // The plugin is unavailable on this platform; manual pairing still works.
  }
}
