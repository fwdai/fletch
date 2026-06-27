import type { ChatItem, DisplayMode, DisplayPolicy } from "./types";

export function modeFor(item: ChatItem, policy: DisplayPolicy): DisplayMode {
  if (item.kind === "notice") {
    const specific = policy[`${item.kind}:${item.subtype}`];
    if (specific) return specific;
  }
  return policy[item.kind] ?? "show";
}

export function applyPolicy(items: ChatItem[], policy: DisplayPolicy): ChatItem[] {
  return items.filter((item) => modeFor(item, policy) !== "hide");
}
