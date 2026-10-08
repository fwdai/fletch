// A confirmed pairing end to end on the mock host: the real store and the real
// protocol client, with no code typed — the phone shows six digits while the
// "Mac" decides, and is connected once it accepts. Its own file because the
// store is a module singleton and this owns its connection from the start.

import { expect, it } from "vitest";
import { DEFAULT_PORT } from "../src/remote";
import { MOCK_CONFIRM_CODE, MOCK_HOST_KEY } from "../src/remote/mock";
import { useStore } from "../src/store";

const state = () => useStore.getState();

it("shows the code while the Mac decides, and connects once it accepts", async () => {
  // As the app does: `init` attaches the store to the client (and, in mock
  // mode, greets the mock host), and the pairing replaces that connection.
  await state().init();
  const seen: (string | null)[] = [];
  const off = useStore.subscribe((s) => {
    if (s.pairCode !== seen.at(-1)) seen.push(s.pairCode);
  });

  await state().connect({ host: "mock", port: DEFAULT_PORT, confirm: true });
  off();

  // Shown during the wait, gone once it is over.
  expect(seen).toContain(MOCK_CONFIRM_CODE);
  expect(state().pairCode).toBeNull();
  expect(state().pairStep).toBeNull();
  expect(state().connection).toBe("connected");
  expect(state().hostKey).toBe(MOCK_HOST_KEY);
  expect(state().hostInfo?.name).toBe("Alex's MacBook Pro");
});
