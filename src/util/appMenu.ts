// Native menu setup. We only customize one thing: a macOS-style "Check for
// Updates…" item in the application submenu (right under "About Fletch"). Since
// assigning a menu replaces the platform default wholesale, we start from that
// default and insert into it rather than rebuilding every standard item.

import { Menu, MenuItem, PredefinedMenuItem, Submenu } from "@tauri-apps/api/menu";
import { useAppStore } from "@/store";

/**
 * Install the app menu with a "Check for Updates…" item. Idempotent-ish and
 * safe to call once at startup: any failure is logged and swallowed so a menu
 * problem can never block the app. No-op if the default menu has no application
 * submenu (e.g. non-macOS platforms without an app menu).
 */
export async function setupAppMenu(): Promise<void> {
  try {
    const menu = await Menu.default();
    const [appMenu] = await menu.items();
    if (!(appMenu instanceof Submenu)) return;

    const separator = await PredefinedMenuItem.new({ item: "Separator" });
    const check = await MenuItem.new({
      text: "Check for Updates…",
      action: () => void useAppStore.getState().runUpdateCheck(),
    });

    // After "About Fletch" (index 0): About / --- / Check for Updates… / …
    await appMenu.insert([separator, check], 1);
    await menu.setAsAppMenu();
  } catch (err) {
    console.warn("Failed to install app menu:", err);
  }
}
