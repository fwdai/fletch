import { useEffect, useMemo, useState } from "react";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Mono } from "@/components/SettingsScreen/CustomAgents/Mono";
import { Chip } from "@/components/ui/Chip";
import { Scrim } from "@/components/ui/Scrim";
import { PROVIDER_DETAIL } from "@/data/providerDetail";
import { PROVIDERS, providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";
import { useAgentAvailability } from "../availability";
import { ModelOptions } from "../ModelOptions";
import { AccountStrip } from "./AccountStrip";
import { AgentList, type OpenSettings } from "./AgentList";
import { ModelFlyout } from "./ModelFlyout";
import { type AccountControl, useAccountView } from "./useAccountView";

export type { AccountControl } from "./useAccountView";

interface Props {
  provider: string;
  model?: string;
  /** Selected custom agent id, if the user picked one rather than a built-in
   *  provider. Drives the chip's identity and the dropdown's active row. */
  customAgentId?: string;
  onChange: (provider: string, model?: string, customAgentId?: string) => void;
  locked?: boolean;
  /** Existing sessions: restrict the picker to changing the MODEL within the
   *  session's current provider. Provider and custom-agent identity are fixed
   *  at spawn, so the dropdown drops the agent/custom-agent sections and shows
   *  only this provider's models. */
  modelOnly?: boolean;
  /** The account the agent runs under, and how to move it. Shown only when
   *  `provider` has more than one account to choose from. */
  account?: AccountControl;
}

/** Agent + model + account picker for the composer. A flat list groups coding
 *  agents and custom agents; hovering a coding agent opens a flyout on the
 *  right for model selection. Clicking an agent row commits its default model;
 *  leaving model unset preserves the provider CLI's default. Selections stay
 *  sticky via `onChange`. When the provider has several accounts, an account
 *  strip closes the menu and the chip names the account.
 *
 *  The menu always opens upward — the composer sits on the bottom edge of the
 *  window. A surface near the top of a panel wants a screen, not a menu that has
 *  to grow the other way (see the Roadmap's `NewChatScreen`). */
export function ModelPicker({
  provider,
  model,
  customAgentId,
  onChange,
  locked = false,
  modelOnly = false,
  account,
}: Props) {
  const [open, setOpen] = useState(false);
  // Coding agent whose model flyout is currently expanded (null = none).
  const [hovered, setHovered] = useState<string | null>(null);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const customAgents = useAppStore((s) => s.customAgents);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  const refreshAccounts = useAppStore((s) => s.refreshProviderAccounts);
  // Whether each agent can actually be spawned right now — installed, and
  // container-ready when the Docker engine is on. Matches the backend refusal
  // in supervisor/lifecycle.rs.
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

  // Looked up against the full library on purpose: it drives the chip, and a
  // selection made elsewhere must still render with its own name rather than
  // silently reading as a bare provider.
  const activeCustom = customAgents.find((a) => a.id === customAgentId);
  // The coding agent whose model panel is currently shown (null = none).
  const hoveredAgent = hovered ? (enabled.find((p) => p.id === hovered) ?? null) : null;

  // Reset the flyout each time the dropdown opens.
  useEffect(() => {
    if (open) setHovered(null);
  }, [open]);
  // And re-list the accounts, so their sign-in states are fresh.
  const offersAccounts = accountView !== null;
  useEffect(() => {
    if (open && offersAccounts) void refreshAccounts();
  }, [open, offersAccounts, refreshAccounts]);

  // Model-only (existing session): changing the model on a provider that bakes
  // it into the process (claude, `restartToApply`) restarts the agent; surface
  // that in the chip tooltip, mirroring the effort chip.
  const restartOnChange =
    modelOnly && !!PROVIDER_DETAIL[provider as keyof typeof PROVIDER_DETAIL]?.restartToApply;
  const chipTip = locked
    ? (activeCustom?.name ?? selected.label)
    : modelOnly
      ? restartOnChange
        ? "Model — changing restarts the agent (rebuilds cache)"
        : accountView
          ? "Model and account"
          : "Model"
      : "Agent and model";

  function pickModel(providerId: string, id: string | undefined) {
    // A model-only pick (existing session) keeps the session's custom-agent
    // identity; a full-picker pick clears it.
    onChange(providerId, id, modelOnly ? customAgentId : undefined);
    setOpen(false);
  }

  function pickCustom(agentId: string, base: string, agentModel: string | null) {
    onChange(base, agentModel ?? undefined, agentId);
    setOpen(false);
  }

  function pickAccount(id: string) {
    account?.onPick(id);
    setOpen(false);
  }

  function openSettings(...args: Parameters<OpenSettings>) {
    setOpen(false);
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

  const accountStrip = accountView && (
    <AccountStrip
      view={accountView}
      // The full menu lists every agent, so it names whose accounts these are.
      title={modelOnly ? "Account" : `Account · ${providerLabel(provider)}`}
      onPick={pickAccount}
      onManage={() => openSettings("providers")}
      onMouseEnter={() => setHovered(null)}
    />
  );

  return (
    <div className="model-picker">
      <Chip
        bordered
        disabled={locked}
        onClick={() => {
          if (!locked) setOpen((v) => !v);
        }}
        tip={chipTip}
        className="model-chip"
      >
        {activeCustom ? (
          <>
            <Mono name={activeCustom.name} hue={activeCustom.color} size={15} />
            <span className="model-chip-agent">{activeCustom.name}</span>
            <span className="model-chip-model truncate">{providerLabel(activeCustom.base)}</span>
          </>
        ) : (
          <>
            <ProviderIcon slug={selected.id} short={selected.short} hue={selected.hue} size={15} />
            <span className="model-chip-agent">{selected.label}</span>
            <span className="model-chip-model truncate">
              {currentModel?.name ?? "Default model"}
            </span>
          </>
        )}
        {accountView && (
          <span className={`model-chip-acct truncate ${accountView.spent ? "is-spent" : ""}`}>
            {accountView.label}
          </span>
        )}
        {!locked && <Icon name="chevD" size={9} />}
      </Chip>

      {open && !modelOnly && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          {/* Transparent wrapper: the main card sits above (z-index 2) the
           *  model side panel (z-index 1) so the panel slides out from
           *  underneath the dropdown rather than floating beside it. */}
          <div className="model-dd-wrap" onMouseLeave={() => setHovered(null)}>
            <div className="model-dd-main">
              <AgentList
                provider={provider}
                customAgentId={customAgentId}
                enabled={enabled}
                installedCount={installedProviders.length}
                availability={availability}
                hovered={hovered}
                setHovered={setHovered}
                onPickModel={pickModel}
                onPickCustom={pickCustom}
                onOpenSettings={openSettings}
              />
              {accountStrip}
            </div>

            {hoveredAgent && (
              <ModelFlyout agent={hoveredAgent}>{renderModelList(hoveredAgent)}</ModelFlyout>
            )}
          </div>
        </>
      )}

      {open && modelOnly && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <div className="model-dd-wrap">
            <div className="model-dd-main">
              <div className="model-sect flex-center text-xs">
                <span>Model</span>
                <span className="model-sect-line" />
              </div>
              {renderModelList(selected)}
              {accountStrip}
            </div>
          </div>
        </>
      )}
    </div>
  );
}
