// Mock data used by the right panel until git/diff/run IPC commands
// land. Each shape is keyed to a component in `components/RightPanel`.
// TODO(amux): wire to real git state via Tauri commands.

export type MockGitState =
  | "clean"
  | "changes"
  | "pushed"
  | "pr-open"
  | "conflicts"
  | "merged";

export const MOCK_GIT_FILES = [
  { path: "src/server/billing/checkout.ts", status: "M", add: 34, rem: 8 },
  { path: "src/server/billing/__tests__/checkout.test.ts", status: "M", add: 12, rem: 2 },
  { path: "src/server/billing/portal.ts", status: "A", add: 28, rem: 0 },
  { path: "src/components/Billing/UpgradeCard.tsx", status: "M", add: 6, rem: 3 },
];

export const MOCK_COMMIT_MESSAGE = {
  title: "feat(billing): route active subscribers to portal session",
  body: "Detect existing subscriptions and short-circuit checkout to the Stripe billing portal. Keeps trial-grant path intact.",
};
