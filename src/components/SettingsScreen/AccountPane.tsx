import { useEffect, useState } from "react";
import { useAppStore } from "../../store";
import { accountInitials } from "../../util/format";
import { SetHead, SetGroup } from "./primitives";

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export function AccountPane() {
  const account = useAppStore((s) => s.account);
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
    (firstName !== account.firstName ||
      lastName !== account.lastName ||
      email !== account.email);
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

  return (
    <div className="set-pane">
      <SetHead eyebrow="Settings · Account" title="Account" />

      <div className="set-profile">
        <div className="set-avatar">{accountInitials(firstName, lastName, email)}</div>
        <div className="set-profile-body">
          <div className="set-profile-name">
            {fullName || <span className="set-profile-empty">Your name</span>}
          </div>
          <div className="set-profile-mail mono">
            {email || "no email set"}
          </div>
        </div>
      </div>

      <SetGroup label="Profile" last>
        <div className="set-form">
          <div className="set-form-grid">
            <label className="set-field">
              <span className="set-field-label">First name</span>
              <input
                className="set-text"
                value={firstName}
                placeholder="Ada"
                spellCheck={false}
                onChange={(e) => {
                  setFirstName(e.target.value);
                  setSavedAt(false);
                }}
              />
            </label>
            <label className="set-field">
              <span className="set-field-label">Last name</span>
              <input
                className="set-text"
                value={lastName}
                placeholder="Lovelace"
                spellCheck={false}
                onChange={(e) => {
                  setLastName(e.target.value);
                  setSavedAt(false);
                }}
              />
            </label>
          </div>
          <label className="set-field">
            <span className="set-field-label">Email</span>
            <input
              className={`set-text ${emailValid ? "" : "invalid"}`}
              type="email"
              value={email}
              placeholder="ada@example.com"
              spellCheck={false}
              autoComplete="email"
              onChange={(e) => {
                setEmail(e.target.value);
                setSavedAt(false);
              }}
            />
            {!emailValid && (
              <span className="set-field-error">Enter a valid email address.</span>
            )}
          </label>

          <div className="set-form-actions">
            {savedAt && !dirty && <span className="set-saved mono">Saved</span>}
            <button
              type="button"
              className="btn-t primary"
              disabled={!canSave}
              onClick={onSave}
            >
              {saving ? "Saving…" : "Save changes"}
            </button>
          </div>
        </div>
      </SetGroup>
    </div>
  );
}
