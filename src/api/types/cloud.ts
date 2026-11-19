// Cloud providers: the services whose machines a workspace can run on instead
// of this Mac. The frontend mirror of what `cloud_providers` answers. All a
// surface knows of a provider is its status here, so none is named in `src/`.

/** A provider's id, as the backend spells it. Any string: a surface renders
 *  the providers `cloud_providers` lists and never branches on one. */
export type CloudProviderId = string;

/** One provider on this Mac, connected or not, and the words it is shown,
 *  connected and explained in. */
export interface CloudProviderStatus {
  id: CloudProviderId;
  /** How the provider is named on screen. */
  label: string;
  /** A token for it is kept in this Mac's keychain. */
  connected: boolean;
  /** Who the token acts for, of the kind `account_label` names. `null` while
   *  not connected. */
  account: string | null;
  /** What the provider calls an account, as a field label: "Organization". */
  account_label: string;
  /** The account an empty account field connects, as its placeholder. */
  account_placeholder: string;
  /** What a token looks like, as the token field's placeholder. */
  token_placeholder: string;
  /** One sentence on where to get a token. */
  token_help: string;
  /** One short sentence on what the provider bills for a workspace there. */
  billing: string;
}
