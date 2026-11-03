import { useEffect, useMemo, useState } from "react";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { Scrim } from "@/components/ui/Scrim";
import { PROVIDER_DETAIL } from "@/data/providerDetail";
import { PROVIDERS, providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";
import { useAgentAvailability } from "../availability";
import { ModelOptions } from "../ModelOptions";
import { RailGroup, RailLink, RailPiece } from "../RunRail";
import { chainTip, compactAccountName, shortModelName } from "../runChain";
import { AccountOptions } from "./AccountOptions";
import { AccountRow } from "./AccountRow";
import { AgentList, type OpenSettings } from "./AgentList";
import { CUSTOM_FLY, CustomAgentOptions } from "./CustomAgentOptions";
import { SideFlyout } from "./SideFlyout";
import { type AccountControl, useAccountView } from "./useAccountView";

export type { AccountControl } from "./useAccountView";

/** `hovered` while the account flyout is the one open; no provider id. */
const ACCOUNT_FLY = "__account";

interface Props {
  provider: string;
  model?: string;
  /** Selected custom agent id, if the user picked one rather than a built-in
   *  provider. Drives the chain's identity and the dropdown's active row. */
  customAgentId?: string;
  onChange: (provider: string, model?: string, customAgentId?: string) => void;
  /** Existing sessions: restrict the picker to changing the MODEL within the
   *  session's current provider. Provider and custom-agent identity are fixed
   *  at spawn, so the chain's tooltip says so, and the dropdown drops the
   *  agent/custom-agent sections to show only this provider's models. */
  modelOnly?: boolean;
  /** The account the agent runs under, and how to move it. Shown only when
   *  `provider` has more than one account to choose from. */
  account?: AccountControl;
}

/** The run rail's agent chain: provider › account › model as ONE piece. The
 *  provider is its icon (a custom agent: its monogram and name), the account
 *  its compact name, the model its name without the vendor; the full names
 *  are the piece's tooltip. One trigger, not three: all of them would open
 *  the same menu, and since its side panel follows the cursor, whichever
 *  panel a click landed on would be lost the moment the cursor crossed the
 *  main card on its way up. No chevron either — the menu opens upward, and
 *  the piece signals itself the way every piece in the rail does: the hover
 *  highlight, the pressed fill while open, and the tooltip's "click to
 *  change".
 *
 *  The menu is the coding agents and one "Custom agents" row; hovering a
 *  coding agent opens a flyout on the right for model selection, hovering
 *  the custom-agents row opens the custom agents configured in Settings the
 *  same way, and the account row opens the accounts. It opens on the current
 *  agent's models — the change made most often — with that agent's row lit.
 *  Clicking an agent row commits its default model; leaving model unset
 *  preserves the provider CLI's default. Selections stay sticky via
 *  `onChange`.
 *
 *  The menu always opens upward — the composer sits on the bottom edge of the
 *  window. A surface near the top of a panel wants a screen, not a menu that has
 *  to grow the other way (see the Roadmap's `NewChatScreen`). */
export function ModelPicker({
  provider,
  model,
  customAgentId,
  onChange,
  modelOnly = false,
  account,
}: Props) {
  const [open, setOpen] = useState(false);
  // Coding agent whose model flyout is currently expanded, ACCOUNT_FLY for
  // the account flyout, or CUSTOM_FLY for the custom agents (null = none).
  const [hovered, setHovered] = useState<string | null>(null);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const customAgents = useAppStore((s) => s.customAgents);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  const refreshAccounts = useAppStore((s) => s.refreshProviderAccounts);
  // Whether each agent can actually be spawned right now — installed, and
  // runnable in the workspace type chosen. Matches the backend refusal in
  // supervisor/lifecycle.rs.
  const availability = useAgentAvailability();
  // A custom agent's `provider` is already its base, so this is always the
  // provider whose accounts apply.
  const accountView = useAccountView(provider, account);

  const selected = PROVIDERS.find((p) => p.id === provider) ?? PROVIDERS[0];
  // Not-installed agents are hidden; agents blocked for other reasons stay, disabled.
  const installedProviders = PROVIDERS.filter((p) => availability(p.id).installed);
  const enabled = installedProviders.filter((p) => providerFlags[p.id] !== false);
  const currentModel = useMemo(() => {
    const list = modelsByAgent[provider] ?? [];
    return list.find((m) => m.id === model);
  }, [model, modelsByAgent, provider]);
  // A pinned id the catalog doesn't know shows as itself rather than as the
  // default it isn't.
  const modelName = model ? (currentModel?.name ?? model) : "Default model";

  // Looked up against the full library on purpose: it drives the chain, and a
  // selection made elsewhere must still render with its own name rather than
  // silently reading as a bare provider.
  const activeCustom = customAgents.find((a) => a.id === customAgentId);
  // The coding agent whose model panel is currently shown (null = none).
  const hoveredAgent = hovered ? (enabled.find((p) => p.id === hovered) ?? null) : null;

  // Re-list the accounts each time the menu opens, so their sign-in states
  // are fresh.
  const offersAccounts = accountView !== null;
  useEffect(() => {
    if (open && offersAccounts) void refreshAccounts();
  }, [open, offersAccounts, refreshAccounts]);

  function toggle() {
    if (open) {
      close();
      return;
    }
    setOpen(true);
    // The full menu lands on the current agent's models; the model-only menu
    // is those models already.
    setHovered(modelOnly ? null : provider);
  }

  function close() {
    setOpen(false);
    setHovered(null);
  }

  // Model-only (existing session): changing the model on a provider that bakes
  // it into the process (claude, `restartToApply`) restarts the agent; surface
  // that in the tooltip, mirroring the effort piece.
  const restartOnChange =
    modelOnly && !!PROVIDER_DETAIL[provider as keyof typeof PROVIDER_DETAIL]?.restartToApply;
  const agentName = activeCustom?.name ?? selected.label;
  // Line one is the chain in full; line two, what a click does.
  const tip = [
    chainTip(
      agentName,
      activeCustom && providerLabel(activeCustom.base),
      accountView?.label,
      accountView?.spent && "limit reached",
      modelName,
    ),
    modelOnly
      ? restartOnChange
        ? "Click to change the model — that restarts the agent (rebuilds cache)"
        : "Click to change the model"
      : "Click to change",
  ].join("\n");

  function pickModel(providerId: string, id: string | undefined) {
    // A model-only pick (existing session) keeps the session's custom-agent
    // identity; a full-picker pick clears it.
    onChange(providerId, id, modelOnly ? customAgentId : undefined);
    close();
  }

  function pickCustom(agentId: string, base: string, agentModel: string | null) {
    onChange(base, agentModel ?? undefined, agentId);
    close();
  }

  function pickAccount(id: string) {
    account?.onPick(id);
    close();
  }

  function openSettings(...args: Parameters<OpenSettings>) {
    close();
    openSettingsScreen(...args);
  }

  /** The list for one provider. In model-only mode it is always the session's
   *  own provider, so its current model highlights even for a custom-agent
   *  session (customAgentId set). */
  function renderModelList(p: (typeof PROVIDERS)[number]) {
    return (
      <ModelOptions
        provider={p}
        model={model}
        isCurrent={modelOnly || (p.id === provider && !customAgentId)}
        onPick={pickModel}
      />
    );
  }

  // The full menu lists every agent, so it names whose accounts these are.
  const accountTitle = modelOnly ? "Account" : `Account · ${providerLabel(provider)}`;
  const accountRow = accountView && (
    <AccountRow
      view={accountView}
      title={accountTitle}
      open={hovered === ACCOUNT_FLY}
      onOpen={() => setHovered(ACCOUNT_FLY)}
    />
  );
  const accountFlyout = accountView && hovered === ACCOUNT_FLY && (
    <SideFlyout
      flyKey={ACCOUNT_FLY}
      icon={<ProviderIcon slug={selected.id} short={selected.short} hue={selected.hue} size={20} />}
      title={providerLabel(provider)}
      tag="account"
    >
      <AccountOptions
        view={accountView}
        onPick={pickAccount}
        onManage={() => openSettings("providers")}
      />
    </SideFlyout>
  );

  return (
    <RailGroup className="model-picker">
      <RailPiece className="run-chain" tip={tip} open={open} onClick={toggle}>
        {activeCustom ? (
          <>
            <Mono name={activeCustom.name} hue={activeCustom.color} size={16} />
            <span className="run-name truncate">{activeCustom.name}</span>
          </>
        ) : (
          <ProviderIcon slug={selected.id} short={selected.short} hue={selected.hue} size={16} />
        )}
        {accountView && (
          <RailLink className="is-acct">
            <span className={`run-acct truncate ${accountView.spent ? "is-spent" : ""}`}>
              {compactAccountName(accountView.label)}
            </span>
          </RailLink>
        )}
        <RailLink>
          <span className="run-model truncate">{shortModelName(modelName)}</span>
        </RailLink>
      </RailPiece>

      {open && !modelOnly && (
        <>
          <Scrim onClose={close} />
          {/* Transparent wrapper: the main card sits above (z-index 2) the
           *  model side panel (z-index 1) so the panel slides out from
           *  underneath the dropdown rather than floating beside it. */}
          <div className="model-dd-wrap" onMouseLeave={() => setHovered(null)}>
            <div className="model-dd-main">
              {accountRow}
              <AgentList
                provider={provider}
                customAgentId={customAgentId}
                enabled={enabled}
                installedCount={installedProviders.length}
                availability={availability}
                hovered={hovered}
                setHovered={setHovered}
                onPickModel={pickModel}
                onOpenSettings={openSettings}
              />
            </div>

            {hovered === CUSTOM_FLY && (
              <SideFlyout
                flyKey={CUSTOM_FLY}
                icon={
                  <span className="model-custom-sum-icon is-head">
                    <Icon name="shapes" size={13} />
                  </span>
                }
                title="Custom agents"
                tag="agent"
              >
                <CustomAgentOptions
                  customAgentId={customAgentId}
                  availability={availability}
                  onPick={pickCustom}
                  onOpenSettings={openSettings}
                />
              </SideFlyout>
            )}
            {hoveredAgent && (
              <SideFlyout
                flyKey={hoveredAgent.id}
                icon={
                  <ProviderIcon
                    slug={hoveredAgent.id}
                    short={hoveredAgent.short}
                    hue={hoveredAgent.hue}
                    size={20}
                  />
                }
                title={hoveredAgent.label}
                tag="model"
              >
                {renderModelList(hoveredAgent)}
              </SideFlyout>
            )}
            {accountFlyout}
          </div>
        </>
      )}

      {open && modelOnly && (
        <>
          <Scrim onClose={close} />
          <div className="model-dd-wrap" onMouseLeave={() => setHovered(null)}>
            <div className="model-dd-main">
              {accountRow}
              {/* Leaving the account row for the models closes its flyout. */}
              <div onMouseEnter={() => setHovered(null)}>
                <div className="model-sect flex-center text-xs">
                  <span>Model</span>
                  <span className="model-sect-line" />
                </div>
                {renderModelList(selected)}
              </div>
            </div>
            {accountFlyout}
          </div>
        </>
      )}
    </RailGroup>
  );
}
