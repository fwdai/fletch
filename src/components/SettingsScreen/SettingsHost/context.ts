import { createContext, useContext } from "react";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";

/** The host Settings shows host-owned settings for, and the way to pick
 *  another. Settings' own: the rest of the window shows what it shows. */
export interface SettingsHost {
  hostId: EnvironmentId;
  choose: (hostId: EnvironmentId) => void;
}

export const SettingsHostContext = createContext<SettingsHost>({
  hostId: LOCAL_ENVIRONMENT_ID,
  choose: () => {},
});

export function useSettingsHost(): SettingsHost {
  return useContext(SettingsHostContext);
}
