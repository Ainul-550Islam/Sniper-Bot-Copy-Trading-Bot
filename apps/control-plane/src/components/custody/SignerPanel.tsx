"use client";

import { useState } from "react";
import { HealthBadge } from "./HealthBadge";
import type {
  CreateCustodySignerInput,
  CustodyProfile,
  CustodySigner,
  SignerHealth,
} from "@/lib/api/custody-api";

type PendingSignerAction = {
  kind: "activate" | "revoke";
  signer: CustodySigner;
};

interface SignerPanelProps {
  profiles: CustodyProfile[];
  signers: CustodySigner[];
  healthBySigner: Record<string, SignerHealth>;
  healthLoadingId: string | null;
  resolvedSigner: CustodySigner | null;
  detailedSigner: CustodySigner | null;
  profileSigners: CustodySigner[] | null;
  profileSignersProfile: CustodyProfile | null;
  canManage: boolean;
  busy: boolean;
  onCreate: (input: CreateCustodySignerInput) => Promise<void>;
  onActivate: (signer: CustodySigner) => Promise<void>;
  onRevoke: (signer: CustodySigner) => Promise<void>;
  onAttachCapability: (signer: CustodySigner, capability: string) => Promise<void>;
  onResolve: (signer: CustodySigner) => Promise<void>;
  onHealth: (signer: CustodySigner) => Promise<void>;
  onGet: (signer: CustodySigner) => Promise<void>;
  onLoadProfileSigners: (profile: CustodyProfile) => Promise<void>;
}

export function SignerPanel({
  profiles,
  signers,
  healthBySigner,
  healthLoadingId,
  resolvedSigner,
  detailedSigner,
  profileSigners,
  profileSignersProfile,
  canManage,
  busy,
  onCreate,
  onActivate,
  onRevoke,
  onAttachCapability,
  onResolve,
  onHealth,
  onGet,
  onLoadProfileSigners,
}: SignerPanelProps) {
  const [profileId, setProfileId] = useState("");
  const [logicalIdentity, setLogicalIdentity] = useState("");
  const [publicAddress, setPublicAddress] = useState("");
  const [capabilities, setCapabilities] = useState("");
  const [providerRef, setProviderRef] = useState("");
  const [capabilitySignerId, setCapabilitySignerId] = useState("");
  const [capability, setCapability] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<PendingSignerAction | null>(null);
  const [browserProfileId, setBrowserProfileId] = useState("");

  async function create(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!profileId || !logicalIdentity.trim() || !publicAddress.trim()) {
      setError("Profile, logical identity and public address are required.");
      return;
    }
    setError(null);
    await onCreate({
      custody_profile_id: profileId,
      logical_identity: logicalIdentity.trim(),
      public_address: publicAddress.trim(),
      capabilities: capabilities.split(",").map((item) => item.trim()).filter(Boolean),
      ...(providerRef.trim() ? { provider_ref: providerRef.trim() } : {}),
    });
    setLogicalIdentity("");
    setPublicAddress("");
    setCapabilities("");
    setProviderRef("");
  }

  async function loadProfileSigners(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const profile = profiles.find((item) => item.id === browserProfileId);
    if (!profile) {
      setError("Select a profile whose signers should be loaded.");
      return;
    }
    setError(null);
    await onLoadProfileSigners(profile);
  }

  async function attach(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const signer = signers.find((item) => item.id === capabilitySignerId);
    if (!signer || !capability.trim()) {
      setError("Select a signer and enter a capability.");
      return;
    }
    setError(null);
    await onAttachCapability(signer, capability.trim());
    setCapability("");
  }

  async function confirmPending() {
    if (!pending) return;
    const action = pending;
    setPending(null);
    if (action.kind === "activate") await onActivate(action.signer);
    else await onRevoke(action.signer);
  }

  return (
    <section className="card" aria-label="Custody signer management">
      <h2>Signer management</h2>
      <p className="muted small">
        Signers are logical identities with public addresses only. Do not enter private keys,
        seed phrases or provider credentials anywhere in this interface.
      </p>
      {!canManage && <p className="muted">Your current role can view signers but cannot change them.</p>}

      <form onSubmit={(event) => void create(event)} className="form" style={{ margin: "1rem 0" }}>
        <div className="form-group">
          <label htmlFor="signer-profile">Custody profile</label>
          <select id="signer-profile" value={profileId} onChange={(event) => setProfileId(event.target.value)} disabled={!canManage || busy} required>
            <option value="">Select a profile</option>
            {profiles.map((profile) => (
              <option key={profile.id} value={profile.id}>{profile.name} ({profile.status})</option>
            ))}
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="signer-identity">Logical identity</label>
          <input id="signer-identity" value={logicalIdentity} onChange={(event) => setLogicalIdentity(event.target.value)} disabled={!canManage || busy} required />
        </div>
        <div className="form-group">
          <label htmlFor="signer-public-address">Public address</label>
          <input id="signer-public-address" value={publicAddress} onChange={(event) => setPublicAddress(event.target.value)} disabled={!canManage || busy} required />
        </div>
        <div className="form-group">
          <label htmlFor="signer-capabilities">Capabilities (comma-separated)</label>
          <input id="signer-capabilities" value={capabilities} onChange={(event) => setCapabilities(event.target.value)} disabled={!canManage || busy} />
        </div>
        <div className="form-group">
          <label htmlFor="signer-provider-ref">Provider reference (optional, non-secret)</label>
          <input id="signer-provider-ref" value={providerRef} onChange={(event) => setProviderRef(event.target.value)} disabled={!canManage || busy} />
        </div>
        <button type="submit" className="primary" disabled={!canManage || busy || profiles.length === 0}>Create signer</button>
      </form>

      <form onSubmit={(event) => void attach(event)} className="form" style={{ margin: "1rem 0" }}>
        <div className="form-group">
          <label htmlFor="capability-signer">Attach capability to signer</label>
          <select id="capability-signer" value={capabilitySignerId} onChange={(event) => setCapabilitySignerId(event.target.value)} disabled={!canManage || busy} required>
            <option value="">Select a signer</option>
            {signers.map((signer) => (
              <option key={signer.id} value={signer.id}>{signer.logical_identity} — {signer.public_address}</option>
            ))}
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="capability-name">Capability</label>
          <input id="capability-name" value={capability} onChange={(event) => setCapability(event.target.value)} disabled={!canManage || busy} required />
        </div>
        <button type="submit" disabled={!canManage || busy || signers.length === 0}>Attach capability</button>
      </form>

      <form onSubmit={(event) => void loadProfileSigners(event)} className="form" style={{ margin: "1rem 0" }}>
        <div className="form-group">
          <label htmlFor="profile-signer-browser">Signers for profile</label>
          <select id="profile-signer-browser" value={browserProfileId} onChange={(event) => setBrowserProfileId(event.target.value)} disabled={busy} required>
            <option value="">Select a profile</option>
            {profiles.map((profile) => (
              <option key={profile.id} value={profile.id}>{profile.name} ({profile.status})</option>
            ))}
          </select>
        </div>
        <button type="submit" disabled={busy || profiles.length === 0}>Load profile signers</button>
      </form>

      {profileSignersProfile && (
        <section aria-label={`Signers for ${profileSignersProfile.name}`} style={{ margin: "1rem 0" }}>
          <h3>Signers for {profileSignersProfile.name}</h3>
          {profileSigners?.length ? (
            <ul>
              {profileSigners.map((signer) => (
                <li key={signer.id}>
                  {signer.logical_identity} — <code>{signer.public_address}</code> ({signer.status})
                </li>
              ))}
            </ul>
          ) : (
            <p className="muted">The server returned no signers for this profile.</p>
          )}
        </section>
      )}

      {error && <p role="alert" className="error">{error}</p>}

      {signers.length === 0 ? (
        <p className="muted">No custody signers returned by the server.</p>
      ) : (
        <div style={{ overflowX: "auto" }}>
          <table>
            <thead>
              <tr>
                <th>Logical identity</th>
                <th>Public address</th>
                <th>Provider</th>
                <th>Status</th>
                <th>Capabilities</th>
                <th>Health</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              {signers.map((signer) => (
                <tr key={signer.id}>
                  <td>{signer.logical_identity}<br /><code className="muted">{signer.id}</code></td>
                  <td><code>{signer.public_address}</code></td>
                  <td>{signer.provider_type}</td>
                  <td><span className="tag">{signer.status}</span></td>
                  <td>{signer.capabilities.join(", ") || "—"}</td>
                  <td><HealthBadge health={healthBySigner[signer.id]} loading={healthLoadingId === signer.id} /></td>
                  <td>
                    <div className="row">
                      <button type="button" className="link" disabled={busy} onClick={() => void onGet(signer)}>Details</button>
                      <button type="button" className="link" disabled={busy} onClick={() => void onHealth(signer)}>Health</button>
                      <button type="button" className="link" disabled={busy} onClick={() => void onResolve(signer)}>Resolve</button>
                      <button type="button" className="link" disabled={!canManage || busy} onClick={() => setPending({ kind: "activate", signer })}>Activate</button>
                      <button type="button" className="link" disabled={!canManage || busy} onClick={() => setPending({ kind: "revoke", signer })}>Revoke</button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {detailedSigner && (
        <div className="notice" style={{ marginTop: "1rem" }}>
          Signer details {detailedSigner.logical_identity}: <code>{detailedSigner.public_address}</code> ({detailedSigner.provider_type}, {detailedSigner.status})
        </div>
      )}

      {resolvedSigner && (
        <div className="notice success" style={{ marginTop: "1rem" }}>
          Resolved signer {resolvedSigner.logical_identity}: <code>{resolvedSigner.public_address}</code> ({resolvedSigner.status})
        </div>
      )}

      {pending && (
        <div role="alertdialog" aria-label="Confirm custody signer action" style={{ marginTop: "1rem", borderLeft: "4px solid var(--bad)", paddingLeft: "1rem" }}>
          <h3>{pending.kind === "activate" ? "Activate" : "Revoke"} this signer?</h3>
          <p className="muted">
            {pending.signer.logical_identity} (<code>{pending.signer.public_address}</code>) is currently {pending.signer.status}.
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

export default SignerPanel;
