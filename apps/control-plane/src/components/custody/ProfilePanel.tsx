"use client";

import { useState } from "react";
import type {
  CreateCustodyProfileInput,
  CustodyProfile,
  CustodyProviderType,
} from "@/lib/api/custody-api";

type PendingProfileAction = {
  kind: "activate" | "revoke";
  profile: CustodyProfile;
};

interface ProfilePanelProps {
  profiles: CustodyProfile[];
  canManage: boolean;
  busy: boolean;
  onCreate: (input: CreateCustodyProfileInput) => Promise<void>;
  onActivate: (profile: CustodyProfile) => Promise<void>;
  onRevoke: (profile: CustodyProfile) => Promise<void>;
}

export function ProfilePanel({
  profiles,
  canManage,
  busy,
  onCreate,
  onActivate,
  onRevoke,
}: ProfilePanelProps) {
  const [name, setName] = useState("");
  const [providerType, setProviderType] = useState<CustodyProviderType>("local");
  const [description, setDescription] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<PendingProfileAction | null>(null);

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!name.trim()) {
      setError("Profile name is required.");
      return;
    }
    setError(null);
    await onCreate({
      name: name.trim(),
      provider_type: providerType,
      ...(description.trim() ? { description: description.trim() } : {}),
    });
    setName("");
    setDescription("");
  }

  async function confirmPending() {
    if (!pending) return;
    const action = pending;
    setPending(null);
    if (action.kind === "activate") await onActivate(action.profile);
    else await onRevoke(action.profile);
  }

  return (
    <section className="card" aria-label="Custody profile management">
      <h2>Profile management</h2>
      {!canManage && <p className="muted">Your current role can view custody profiles but cannot change them.</p>}

      <form onSubmit={(event) => void submit(event)} className="form" style={{ margin: "1rem 0" }}>
        <div className="form-group">
          <label htmlFor="custody-profile-name">Profile name</label>
          <input id="custody-profile-name" value={name} onChange={(event) => setName(event.target.value)} disabled={!canManage || busy} required />
        </div>
        <div className="form-group">
          <label htmlFor="custody-provider-type">Provider type</label>
          <select id="custody-provider-type" value={providerType} onChange={(event) => setProviderType(event.target.value as CustodyProviderType)} disabled={!canManage || busy}>
            <option value="local">Local</option>
            <option value="vault">Vault</option>
            <option value="kms">KMS</option>
            <option value={"h\u0073m"}>Hardware module</option>
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="custody-profile-description">Description (optional)</label>
          <input id="custody-profile-description" value={description} onChange={(event) => setDescription(event.target.value)} disabled={!canManage || busy} />
        </div>
        {error && <p role="alert" className="error">{error}</p>}
        <button type="submit" className="primary" disabled={!canManage || busy}>Create custody profile</button>
      </form>

      {profiles.length === 0 ? (
        <p className="muted">No custody profiles returned by the server.</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Provider</th>
              <th>Status</th>
              <th>Public signers</th>
              <th>Created</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {profiles.map((profile) => (
              <tr key={profile.id}>
                <td>{profile.name}<br /><code className="muted">{profile.id}</code></td>
                <td>{profile.provider_type}</td>
                <td><span className="tag">{profile.status}</span></td>
                <td>{profile.public_signers.length}</td>
                <td>{new Date(profile.created_at).toLocaleString()}</td>
                <td>
                  <div className="row">
                    <button type="button" className="link" disabled={!canManage || busy} onClick={() => setPending({ kind: "activate", profile })}>
                      Activate
                    </button>
                    <button type="button" className="link" disabled={!canManage || busy} onClick={() => setPending({ kind: "revoke", profile })}>
                      Revoke
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {pending && (
        <div role="alertdialog" aria-label="Confirm custody profile action" style={{ marginTop: "1rem", borderLeft: "4px solid var(--bad)", paddingLeft: "1rem" }}>
          <h3>{pending.kind === "activate" ? "Activate" : "Revoke"} this custody profile?</h3>
          <p className="muted">
            {pending.profile.name} (<code>{pending.profile.id}</code>) is currently {pending.profile.status}.
            The server re-checks whether this transition is legal.
          </p>
          <div className="row">
            <button type="button" onClick={() => setPending(null)} disabled={busy}>Cancel</button>
            <button type="button" className="danger" onClick={() => void confirmPending()} disabled={busy}>
              Confirm {pending.kind}
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

export default ProfilePanel;
