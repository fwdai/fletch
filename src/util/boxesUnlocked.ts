import { useAppStore } from "@/store";

/** Whether this user may see the isolated-VM controls at all: the box
 *  sandbox is rolled out behind the same unlock as Settings › Developer (a
 *  dev build, or an admin in production) until it is released to everyone.
 *  The engine side is the `boxes_enabled` switch, off by default
 *  (`boxes::settings::DEFAULT_ENABLED`); this only decides what is drawn.
 *  Releasing means flipping that default and deleting this gate — the
 *  automatic engine, the background download and every surface that reads
 *  the switch are already in place for everyone. */
export function useBoxesUnlocked(): boolean {
  const admin = useAppStore((s) => s.admin);
  return import.meta.env.DEV || admin;
}
