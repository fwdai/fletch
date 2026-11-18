import type { ComponentType } from "react";
import type { CloudProviderId, CloudProviderStatus } from "@/api";
import { FlyConnectForm } from "./FlyConnectForm";

/** What the pane knows of a provider beyond what the backend says. */
interface ProviderUi {
  /** How it is connected: what it asks for differs per provider. */
  Form: ComponentType<{ provider: CloudProviderStatus }>;
  /** What its `account` is, for the connected line. */
  account: string;
}

const PROVIDER_UI: Record<CloudProviderId, ProviderUi> = {
  fly: { Form: FlyConnectForm, account: "organization" },
};

/** The pane's knowledge of `id`, if it has any: a provider the backend lists
 *  before this pane knows it still gets its row, only no form. */
export function providerUi(id: string): ProviderUi | undefined {
  return (PROVIDER_UI as Partial<Record<string, ProviderUi>>)[id];
}

/** Where a connected provider stands, in words. */
export function connectedLine({ id, account }: CloudProviderStatus): string {
  if (!account) return "Connected";
  const noun = providerUi(id)?.account;
  return noun ? `Connected to the ${account} ${noun}` : `Connected as ${account}`;
}
