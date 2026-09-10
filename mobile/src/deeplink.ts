// `fletch://pair?…` arriving from a QR reader or a link on the host. Inside
// Tauri the deep-link plugin delivers it; in a browser the same URL can be
// pasted into the Pair screen, so there is nothing to register.

import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { parsePairUrl } from "./remote";
import { inTauri } from "./remote/ws";
import { useStore } from "./store";

/** Pair from the first `fletch://pair` link in `urls`, ignoring anything else
 *  the app may have been opened with. */
function handle(urls: string[]): void {
  for (const url of urls) {
    const target = parsePairUrl(url);
    if (target) {
      useStore.getState().pairFromLink(target);
      return;
    }
  }
}

export async function registerDeepLinks(): Promise<void> {
  if (!inTauri()) return;
  try {
    await onOpenUrl(handle);
  } catch {
    // The plugin is unavailable on this platform; manual pairing still works.
    return;
  }
  try {
    // `onOpenUrl` is nothing but a listener for `deep-link://new-url`, and iOS
    // hands the launch URL to the plugin — which emits it once, immediately —
    // long before this webview exists to hear it. A cold start from a scanned
    // QR is therefore delivered to no one: the app opens on the Pair screen
    // and sits there. `getCurrent` is the plugin's app-load counterpart and
    // the only way to read that URL, so it is asked here, after the listener
    // is in place so a link arriving in between cannot fall through the gap.
    // `pairFromLink` drops the duplicate when both do deliver.
    handle((await getCurrent()) ?? []);
  } catch {
    // Nothing pending. A link tapped later still reaches the listener above.
  }
}
