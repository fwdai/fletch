// The app's screens and sheets as one host sees them. The stack and the sheet
// are the app's (store/index), shared by every host, and every screen or sheet
// opened for a host names it (`withHost`). So what a host does to them — open
// one, ask what of its own is up, close or pop its own — is decided here by
// that one name, and never touches what another host has open.

import { appStore, type NavItem, type ScreenName, type SheetName, type SheetState } from "./index";

type Props = Record<string, string>;

/** `props` naming `hostKey`'s host, for a screen or sheet opened on it: it acts
 *  on that host whichever is on screen by then. */
export const withHost = (hostKey: string, props: Props = {}): Props => ({ ...props, hostKey });

/** Whether a screen on the stack, or the sheet, is `hostKey`'s. */
const isOn = (hostKey: string | null, item: { props: Props } | null | undefined): boolean =>
  item?.props.hostKey === hostKey;

/** `hostKey`'s screens on the stack, bottom first. */
const screensOf = (hostKey: string | null): NavItem[] =>
  appStore.getState().nav.filter((n) => isOn(hostKey, n));

/** Whether a screen of `hostKey`'s agent `agentId` is on the stack — the one
 *  case a transcript read for it has anyone to show it to. Agent ids recur
 *  across hosts, so the host has to match as well as the id. */
export const isAgentOpen = (hostKey: string | null, agentId: string): boolean =>
  screensOf(hostKey).some((n) => n.props.agentId === agentId);

/** The agent of `hostKey`'s top-most screen that shows one, if any. */
export const openAgentOf = (hostKey: string | null): string | undefined =>
  screensOf(hostKey)
    .filter((n) => n.props.agentId)
    .at(-1)?.props.agentId;

/** Whether `hostKey`'s `screen` is on the stack. */
export const isScreenOpen = (hostKey: string | null, screen: ScreenName): boolean =>
  screensOf(hostKey).some((n) => n.screen === screen);

/** The sheet, while it is up as `hostKey`'s `name`; else null. */
export function sheetOf(hostKey: string | null, name: SheetName): SheetState | null {
  const { sheet } = appStore.getState();
  return sheet?.open && sheet.name === name && isOn(hostKey, sheet) ? sheet : null;
}

/** Open `screen` on `hostKey`'s host. */
export const pushOn = (hostKey: string, screen: ScreenName, props?: Props): void =>
  appStore.getState().push(screen, withHost(hostKey, props));

/** Close the sheet if it is `hostKey`'s — and with `name`, only if it is that
 *  one. For an action finishing late, which must not dismiss what the user
 *  opened since, on this host or another. */
export function closeSheetOf(hostKey: string | null, name?: SheetName): void {
  const { sheet, closeSheet } = appStore.getState();
  if (sheet?.open && isOn(hostKey, sheet) && (!name || sheet.name === name)) closeSheet();
}

/** Take the screen of `hostKey`'s agent `agentId` off the stack, if it is the
 *  one on top. */
export function popAgent(hostKey: string | null, agentId: string): void {
  const { nav, pop } = appStore.getState();
  const top = nav.filter((i) => i.phase !== "leave").at(-1);
  if (top?.screen === "agent" && top.props.agentId === agentId && isOn(hostKey, top)) pop();
}
