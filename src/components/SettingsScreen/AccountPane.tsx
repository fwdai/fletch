import { useEffect, useState } from "react";
import { Avatar } from "@/components/Avatar";
import { Button } from "@/components/ui/Button";
import { TextInput } from "@/components/ui/TextInput";
import { useAppStore } from "@/store";
import { accountInitials } from "@/util/format";
import { displayIdentity } from "@/util/identity";
import { DevToolsStatus } from "./DevToolsStatus";
import { GithubConnection } from "./GithubConnection";
import { LinearConnection } from "./LinearConnection";
import { SetGroup, SetHead } from "./primitives";

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export function AccountPane() {
  const account = useAppStore((s) => s.account);
  const gitIdentity = useAppStore((s) => s.gitIdentity);
  const saveAccount = useAppStore((s) => s.saveAccount);

  const [firstName, setFirstName] = useState("");
  const [lastName, setLastName] = useState("");
  const [email, setEmail] = useState("");
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState(false);

  // Hydrate local fields once the account row loads (or changes identity).
  useEffect(() => {
    setFirstName(account?.firstName ?? "");
    setLastName(account?.lastName ?? "");
    setEmail(account?.email ?? "");
  }, [account?.id]);

  const dirty =
    !!account &&
    (firstName !== account.firstName || lastName !== account.lastName || email !== account.email);
  const emailValid = email.trim() === "" || EMAIL_RE.test(email.trim());
  const canSave = dirty && emailValid && !saving;

  const onSave = async () => {
    if (!canSave) return;
    setSaving(true);
    setSavedAt(false);
    await saveAccount({
      firstName: firstName.trim(),
      lastName: lastName.trim(),
      email: email.trim(),
    });
    setSaving(false);
    setSavedAt(true);
  };

  const fullName = `${firstName} ${lastName}`.trim();
  // The header follows what is typed; with the fields empty it shows the git
  // identity the sidebar is showing, so the user sees where that came from.
  const fallback = displayIdentity(null, gitIdentity);
  const fromGit = !fullName && !email && fallback.source === "git";
  const headName = fullName || (fromGit ? fallback.name : "");
  const headEmail = email || (fromGit ? fallback.email : "");
  const [gitFirst = "", ...gitRest] = (gitIdentity?.name ?? "").trim().split(/\s+/);

  return (
    <div className="set-pane">
      <SetHead eyebrow="Settings · Account" title="Account" />

      <div className="set-profile flex-center">
        <Avatar
          className="set-avatar flex-center text-lg"
          avatarUrl={account?.avatarUrl ?? null}
          initials={
            fullName || email ? accountInitials(firstName, lastName, email) : fallback.initials
          }
          glyphSize={24}
          alt={headName}
        />
        <div className="set-profile-body">
          <div className="set-profile-name text-lg">
            {headName || <span className="set-profile-empty">Your name</span>}
          </div>
          <div className="set-profile-mail mono text-base">{headEmail || "no email set"}</div>
          {fromGit && (
            <div className="set-profile-hint text-sm">
              From your git config. Fill in the profile below to override it.
            </div>
          )}
        </div>
      </div>

      <SetGroup label="Profile">
        <div className="set-form">
          <div className="set-form-grid">
            <label className="set-field">
              <span className="set-field-label text-sm">First name</span>
              <TextInput
                value={firstName}
                placeholder={gitFirst || "Ada"}
                spellCheck={false}
                onChange={(e) => {
                  setFirstName(e.target.value);
                  setSavedAt(false);
                }}
              />
            </label>
            <label className="set-field">
              <span className="set-field-label text-sm">Last name</span>
              <TextInput
                value={lastName}
                placeholder={gitRest.join(" ") || "Lovelace"}
                spellCheck={false}
                onChange={(e) => {
                  setLastName(e.target.value);
                  setSavedAt(false);
                }}
              />
            </label>
          </div>
          <label className="set-field">
            <span className="set-field-label text-sm">Email</span>
            <TextInput
              invalid={!emailValid}
              type="email"
              value={email}
              placeholder={gitIdentity?.email || "ada@example.com"}
              spellCheck={false}
              autoComplete="email"
              onChange={(e) => {
                setEmail(e.target.value);
                setSavedAt(false);
              }}
            />
            {!emailValid && (
              <span className="set-field-error text-sm">Enter a valid email address.</span>
            )}
          </label>

          <div className="set-form-actions flex-center">
            {savedAt && !dirty && <span className="set-saved mono text-sm">Saved</span>}
            <Button variant="primary" disabled={!canSave} onClick={onSave}>
              {saving ? "Saving…" : "Save changes"}
            </Button>
          </div>
        </div>
      </SetGroup>

      <SetGroup label="GitHub">
        <GithubConnection />
      </SetGroup>

      <SetGroup label="Linear">
        <LinearConnection />
      </SetGroup>

      <SetGroup label="Developer tools" last>
        <DevToolsStatus />
      </SetGroup>
    </div>
  );
}
