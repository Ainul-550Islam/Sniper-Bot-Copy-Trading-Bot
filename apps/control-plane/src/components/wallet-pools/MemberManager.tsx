"use client";

import { useState } from "react";
import type { CustodySigner } from "@/lib/api/custody-api";
import type {
  AddWalletPoolMemberInput,
  WalletPoolDetail,
  WalletPoolMember,
} from "@/lib/api/wallet-pools-api";

interface MemberManagerProps {
  pool: WalletPoolDetail;
  signers: CustodySigner[];
  canManage: boolean;
  busy: boolean;
  onAdd: (input: AddWalletPoolMemberInput) => Promise<void>;
  onRemove: (member: WalletPoolMember) => Promise<void>;
}

export function MemberManager({
  pool,
  signers,
  canManage,
  busy,
  onAdd,
  onRemove,
}: MemberManagerProps) {
  const [signerId, setSignerId] = useState("");
  const [weight, setWeight] = useState("1");
  const [error, setError] = useState<string | null>(null);
  const [removeTarget, setRemoveTarget] = useState<WalletPoolMember | null>(null);
  const activeSigners = signers.filter((signer) => signer.status === "active");

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const parsedWeight = Number(weight);
    if (!signerId) {
      setError("Select an active custody signer.");
      return;
    }
    if (!Number.isInteger(parsedWeight) || parsedWeight < 1 || parsedWeight > 1000) {
      setError("Weight must be a whole number between 1 and 1000.");
      return;
    }
    setError(null);
    await onAdd({ custody_signer_id: signerId, weight: parsedWeight });
    setSignerId("");
    setWeight("1");
  }

  return (
    <section className="card" aria-label={`Members of ${pool.name}`}>
      <h2>Pool members</h2>
      <p className="muted small">
        Members are this organization&apos;s custody signers, shown by logical identity and
        public address only.
      </p>

      <form onSubmit={(event) => void submit(event)} className="form" style={{ margin: "1rem 0" }}>
        <div className="form-group">
          <label htmlFor="pool-member-signer">Custody signer</label>
          <select
            id="pool-member-signer"
            value={signerId}
            onChange={(event) => setSignerId(event.target.value)}
            disabled={!canManage || busy || pool.status === "disabled"}
            required
          >
            <option value="">Select an active signer</option>
            {activeSigners.map((signer) => (
              <option key={signer.id} value={signer.id}>
                {signer.logical_identity} — {signer.public_address}
              </option>
            ))}
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="pool-member-weight">Weight</label>
          <input
            id="pool-member-weight"
            type="number"
            min="1"
            max="1000"
            step="1"
            value={weight}
            onChange={(event) => setWeight(event.target.value)}
            disabled={!canManage || busy || pool.status === "disabled"}
            required
          />
        </div>
        {activeSigners.length === 0 && (
          <p className="muted">No active custody signers are available to add.</p>
        )}
        {error && <p role="alert" className="error">{error}</p>}
        <button type="submit" className="primary" disabled={!canManage || busy || pool.status === "disabled" || activeSigners.length === 0}>
          Add member
        </button>
      </form>

      {pool.members.length === 0 ? (
        <p className="muted">This pool has no active members.</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Logical identity</th>
              <th>Public address</th>
              <th>Weight</th>
              <th>Status</th>
              <th>Added</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {pool.members.map((member) => (
              <tr key={member.id}>
                <td>{member.logical_identity}</td>
                <td><code>{member.public_address}</code></td>
                <td>{member.weight}</td>
                <td>{member.status}</td>
                <td>{new Date(member.added_at).toLocaleString()}</td>
                <td>
                  <button
                    type="button"
                    className="link"
                    disabled={!canManage || busy}
                    onClick={() => setRemoveTarget(member)}
                  >
                    Remove
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {removeTarget && (
        <div role="alertdialog" aria-label="Confirm wallet pool member removal" style={{ marginTop: "1rem", borderLeft: "4px solid var(--bad)", paddingLeft: "1rem" }}>
          <h3>Remove this pool member?</h3>
          <p className="muted">
            {removeTarget.logical_identity} (<code>{removeTarget.public_address}</code>) will no
            longer be selected from this pool for new orders.
          </p>
          <div className="row">
            <button type="button" onClick={() => setRemoveTarget(null)} disabled={busy}>Keep member</button>
            <button
              type="button"
              className="danger"
              disabled={busy}
              onClick={() => {
                const target = removeTarget;
                setRemoveTarget(null);
                void onRemove(target);
              }}
            >
              Confirm removal
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

export default MemberManager;
