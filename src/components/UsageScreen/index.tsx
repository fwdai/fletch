import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { UsagePane } from "./UsagePane";

/** Full-screen usage report. Rendered in place of the workspace panes while
 *  `usageScreenOpen` is true, on the same shell as the settings screen minus
 *  its nav: one report, so there is nothing to navigate between. */
export function UsageScreen() {
  const close = useAppStore((s) => s.closeUsageScreen);

  return (
    <div className="set-screen usg-screen">
      <div className="set-main">
        <div className="set-content">
          <button className="set-back flex-center text-base" onClick={close}>
            <Icon name="chevL" size={13} />
            <span>Back to app</span>
          </button>
          <UsagePane />
        </div>
      </div>
    </div>
  );
}
