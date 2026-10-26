"use client";

import { useState } from "react";
import type {
  UpdateWalletPoolInput,
  WalletPoolAllocation,
  WalletPoolDetail,
  WalletPoolStatus,
} from "@/lib/api/wallet-pools-api";

interface PoolDetailProps {
  pool: WalletPoolDetail;
  canManage: boolean;
  busy: boolean;
  onUpdate: (input: UpdateWalletPoolInput) => Promise<void>;
}

export function PoolDetailPanel({ pool, canManage, busy, onUpdate }: PoolDetailProps) {
  const [name, setName] = useState(pool.name);
  const [allocation, setAllocation] = useState<WalletPoolAllocation>(pool.allocation);
  const [status, setStatus] = useState<WalletPoolStatus>(pool.status);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!name.trim()) {
      setError("Pool name is required.");
      return;
    }
    setError(null);
    await onUpdate({ name: name.trim(), allocation, status });
  }

  return (
    <section className="card" aria-label={`Wallet pool ${pool.name}`}>
      <div className="row-between">
        <div>
          <h2>{pool.name}</h2>
          <p className="muted small">
            <code>{pool.id}</code> · rotation cursor {pool.rotation_cursor} · created {new Date(pool.created_at).toLocaleString()}
          </p>
        </div>
        <span className="tag">{pool.status}</span>
      </div>

      <form onSubmit={(event) => void submit(event)} className="form" style={{ marginTop: "1rem" }}>
        <div className="form-group">
          <label htmlFor="pool-detail-name">Name</label>
          <input id="pool-detail-name" value={name} onChange={(event) => setName(event.target.value)} disabled={!canManage || busy} required maxLength={64} />
        </div>
        <div className="form-group">
          <label htmlFor="pool-detail-allocation">Allocation</label>
          <select id="pool-detail-allocation" value={allocation} onChange={(event) => setAllocation(event.target.value as WalletPoolAllocation)} disabled={!canManage || busy}>
            <option value="round_robin">Round robin</option>
            <option value="weighted_split">Weighted split</option>
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="pool-detail-status">Status</label>
          <select id="pool-detail-status" value={status} onChange={(event) => setStatus(event.target.value as WalletPoolStatus)} disabled={!canManage || busy}>
            <option value="active">Active</option>
            <option value="disabled">Disabled</option>
          </select>
        </div>
        {error && <p role="alert" className="error">{error}</p>}
        <button type="submit" className="primary" disabled={!canManage || busy}>
          {busy ? "Saving…" : "Save pool changes"}
        </button>
      </form>
    </section>
  );
}

export default PoolDetailPanel;
