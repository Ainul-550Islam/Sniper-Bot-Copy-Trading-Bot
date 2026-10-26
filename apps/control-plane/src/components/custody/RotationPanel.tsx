"use client";

import { useState } from "react";
import type {
  CreateCustodyRotationInput,
  CustodyProfile,
  CustodyRotation,
  CustodySigner,
} from "@/lib/api/custody-api";

type PendingRotationAction = {
  kind: "activate" | "revoke";
  rotation: CustodyRotation;
};

interface RotationPanelProps {
  profiles: CustodyProfile[];
  signers: CustodySigner[];
  rotations: CustodyRotation[];
  canManage: boolean;
  busy: boolean;
  onCreate: (input: CreateCustodyRotationInput) => Promise<void>;
  onLoad: (id: string) => Promise<void>;
  onActivate: (rotation: CustodyRotation) => Promise<void>;
  onRevoke: (rotation: CustodyRotation) => Promise<void>;
}

export function RotationPanel({
  profiles,
  signers,
  rotations,
  canManage,
  busy,
  onCreate,
  onLoad,
  onActivate,
  onRevoke,
}: RotationPanelProps) {
  const [profileId, setProfileId] = useState("");
  const [oldSignerId, setOldSignerId] = useState("");
  const [newSignerId, setNewSignerId] = useState("");
  const [lookupId, setLookupId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<PendingRotationAction | null>(null);
  const activeProfiles = profiles.filter((profile) => profile.status === "active");
  const selectedProfile = activeProfiles.find((profile) => profile.id === profileId);
  const rotationSigners = selectedProfile
    ? signers.filter((signer) =>
        selectedProfile.public_signers.some((profileSigner) => profileSigner.id === signer.id),
      )
    : [];

  async function create(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!profileId || !oldSignerId || !newSignerId || oldSignerId === newSignerId) {
      setError("Select a profile and two different signers.");
      return;
    }
    setError(null);
    await onCreate({
      profile_id: profileId,
      old_signer_id: oldSignerId,
      new_signer_id: newSignerId,
    });
  }

  async function load(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!lookupId.trim()) {
      setError("Enter a rotation id to load.");
      return;
    }
    setError(null);
    await onLoad(lookupId.trim());
  }

  async function confirmPending() {
    if (!pending) return;
    const action = pending;
    setPending(null);
    if (action.kind === "activate") await onActivate(action.rotation);
    else await onRevoke(action.rotation);
  }

  return (
    <section className="card" aria-label="Custody rotation management">
      <h2>Rotation management</h2>
      <p className="muted small">
        The server re-verifies the profile and both signers before a rotation is recorded.
        Rotations shown here are only those created or loaded from the server in this session.
      </p>
      {!canManage && <p className="muted">Your current role can view loaded rotations but cannot change them.</p>}

      <form onSubmit={(event) => void create(event)} className="form" style={{ margin: "1rem 0" }}>
        <div className="form-group">
          <label htmlFor="rotation-profile">Custody profile</label>
          <select id="rotation-profile" value={profileId} onChange={(event) => { setProfileId(event.target.value); setOldSignerId(""); setNewSignerId(""); }} disabled={!canManage || busy} required>
            <option value="">Select an active profile</option>
            {activeProfiles.map((profile) => (
              <option key={profile.id} value={profile.id}>{profile.name} ({profile.status})</option>
            ))}
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="rotation-old-signer">Current signer</label>
          <select id="rotation-old-signer" value={oldSignerId} onChange={(event) => setOldSignerId(event.target.value)} disabled={!canManage || busy} required>
            <option value="">Select current signer</option>
            {rotationSigners.map((signer) => (
              <option key={signer.id} value={signer.id}>{signer.logical_identity} — {signer.public_address}</option>
            ))}
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="rotation-new-signer">Replacement signer</label>
          <select id="rotation-new-signer" value={newSignerId} onChange={(event) => setNewSignerId(event.target.value)} disabled={!canManage || busy} required>
            <option value="">Select replacement signer</option>
            {rotationSigners.filter((signer) => signer.id !== oldSignerId).map((signer) => (
              <option key={signer.id} value={signer.id}>{signer.logical_identity} — {signer.public_address}</option>
            ))}
          </select>
        </div>
        <button type="submit" className="primary" disabled={!canManage || busy || activeProfiles.length === 0 || rotationSigners.length < 2}>
          Create rotation
        </button>
      </form>

      <form onSubmit={(event) => void load(event)} className="form" style={{ margin: "1rem 0" }}>
        <div className="form-group">
          <label htmlFor="rotation-id">Rotation ID</label>
          <input id="rotation-id" value={lookupId} onChange={(event) => setLookupId(event.target.value)} disabled={busy} required />
        </div>
        <button type="submit" disabled={busy}>Load rotation</button>
      </form>

      {error && <p role="alert" className="error">{error}</p>}

      {rotations.length === 0 ? (
        <p className="muted">No rotation has been created or loaded in this session.</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Rotation</th>
              <th>Profile</th>
              <th>Old signer</th>
              <th>New signer</th>
              <th>State</th>
              <th>Created</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {rotations.map((rotation) => (
              <tr key={rotation.id}>
                <td><code>{rotation.id}</code></td>
                <td><code>{rotation.profile_id}</code></td>
                <td><code>{rotation.old_signer}</code></td>
                <td><code>{rotation.new_signer}</code></td>
                <td><span className="tag">{rotation.state}</span></td>
                <td>{new Date(rotation.created_at).toLocaleString()}</td>
                <td>
                  <div className="row">
                    <button type="button" className="link" disabled={!canManage || busy} onClick={() => setPending({ kind: "activate", rotation })}>Activate</button>
                    <button type="button" className="link" disabled={!canManage || busy} onClick={() => setPending({ kind: "revoke", rotation })}>Revoke</button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {pending && (
        <div role="alertdialog" aria-label="Confirm custody rotation action" style={{ marginTop: "1rem", borderLeft: "4px solid var(--bad)", paddingLeft: "1rem" }}>
          <h3>{pending.kind === "activate" ? "Activate" : "Revoke"} this rotation?</h3>
          <p className="muted">Rotation <code>{pending.rotation.id}</code> is currently {pending.rotation.state}.</p>
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

export default RotationPanel;
