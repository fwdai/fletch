// Cloud providers: the services whose machines a workspace can run on instead
// of this Mac. The frontend mirror of what `cloud_providers` answers.

/** A provider the backend knows. More will join (Daytona, E2B), so a surface
 *  renders the providers `cloud_providers` lists, never this union. */
export type CloudProviderId = "fly";

/** One provider on this Mac, connected or not. */
export interface CloudProviderStatus {
  id: CloudProviderId;
  /** How the provider is named on screen: "Fly.io". */
  label: string;
  /** A token for it is kept in this Mac's keychain. */
  connected: boolean;
  /** Who the token acts for: for Fly, the organization's slug. `null` while
   *  not connected. */
  account: string | null;
}
