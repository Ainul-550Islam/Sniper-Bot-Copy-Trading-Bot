"use client";

import { useState } from "react";
import type {
  CreateWalletPoolInput,
  WalletPoolAllocation,
  WalletPoolSummary,
} from "@/lib/api/wallet-pools-api";

interface PoolListProps {
  pools: WalletPoolSummary[];
  selectedPoolId: string | null;
  canManage: boolean;
  creating: boolean;
  onCreate: (input: CreateWalletPoolInput) => Promise<void>;
  onSelect: (pool: WalletPoolSummary) => void;
}

export function PoolList({
  pools,
  selectedPoolId,
  canManage,
  creating,
  onCreate,
  onSelect,
}: PoolListProps) {
  const [name, setName] = useState("");
  const [allocation, setAllocation] = useState<WalletPoolAllocation>("round_robin");
  const [error, setError] = useState<string | null>(null);

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!name.trim()) {
      setError("Pool name is required.");
      return;
    }
    setError(null);
    await onCreate({ name: name.trim(), allocation });
    setName("");
  }

  return (
    <section className="card" aria-label="Wallet pools">
      <h2>Wallet pools</h2>
      {!canManage && <p className="muted">Your current role can view wallet pools but cannot change them.</p>}

      <form onSubmit={(event) => void submit(event)} className="form" style={{ marginBottom: "1.5rem" }}>
        <div className="form-group">
          <label htmlFor="pool-name">New pool name</label>
          <input id="pool-name" value={name} onChange={(event) => setName(event.target.value)} disabled={!canManage || creating} required maxLength={64} />
        </div>
        <div className="form-group">
          <label htmlFor="pool-allocation">Allocation</label>
          <select id="pool-allocation" value={allocation} onChange={(event) => setAllocation(event.target.value as WalletPoolAllocation)} disabled={!canManage || creating}>
            <option value="round_robin">Round robin</option>
            <option value="weighted_split">Weighted split</option>
          </select>
        </div>
        {error && <p role="alert" className="error">{error}</p>}
        <button type="submit" className="primary" disabled={!canManage || creating}>
          {creating ? "Creating…" : "Create wallet pool"}
        </button>
      </form>

      {pools.length === 0 ? (
        <p className="muted">No wallet pools recorded for this organization yet.</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Allocation</th>
              <th>Status</th>
              <th>Active members</th>
              <th>Created</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {pools.map((pool) => (
              <tr key={pool.id} aria-current={pool.id === selectedPoolId ? "true" : undefined}>
                <td>{pool.name}</td>
                <td>{pool.allocation}</td>
                <td><span className="tag">{pool.status}</span></td>
                <td>{pool.active_members}</td>
                <td>{new Date(pool.created_at).toLocaleString()}</td>
                <td>
                  <button type="button" className="link" onClick={() => onSelect(pool)}>
                    {pool.id === selectedPoolId ? "Selected" : "View"}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

export default PoolList;
