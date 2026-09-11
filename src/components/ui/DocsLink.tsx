import { open as openExternal } from "@tauri-apps/plugin-shell";
import { Icon } from "@/components/Icon";

/** Small accent "→ external docs" affordance: opens `url` in the user's own
 *  browser (never an in-app webview). Used wherever the app offers vendor
 *  instructions instead of doing the thing itself — the readiness check,
 *  onboarding's setup steps, and the providers pane's install guide. */
export function DocsLink({ url, label = "Setup guide" }: { url: string; label?: string }) {
  return (
    <button
      type="button"
      className="rdy-docs iflex-center text-sm"
      onClick={() => void openExternal(url)}
    >
      {label}
      <Icon name="external" size={10} />
    </button>
  );
}
