"use client";

import { useState } from "react";
import {
  addKey,
  revokeKey,
  type OperatorKey,
} from "@/lib/api/operator-api";

function getErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}

export interface KeyListProps {
  keys: OperatorKey[];
  onRefresh: () => Promise<void> | void;
}

export function KeyList({ keys, onRefresh }: KeyListProps) {
  const [label, setLabel] = useState("");
  const [role, setRole] = useState("operator");
  const [secretKey, setSecretKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);

  const handleAddKey = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!label.trim() || !secretKey.trim()) return;
    if (secretKey.length < 24) {
      setError("Key must be at least 24 characters long");
      return;
    }

    setBusy(true);
    setError(null);
    setSuccess(null);

    try {
      const res = await addKey({
        label: label.trim(),
        key: secretKey.trim(),
        role,
      });

      // Clear the secret field immediately after submit
      setSecretKey("");
      setLabel("");
      setSuccess(`Key '${res.label}' added successfully with role '${res.role}'. Hash: ${res.hash || res.key_hash || "created"}`);
      await onRefresh();
    } catch (err) {
      setError(getErrorMessage(err));
      // Clear secret on failure as well
      setSecretKey("");
    } finally {
      setBusy(false);
    }
  };

  const handleRevoke = async (hash: string) => {
    if (!confirm(`Are you sure you want to revoke key with hash ${hash}?`)) return;
    setBusy(true);
    setError(null);
    try {
      await revokeKey(hash);
      setSuccess(`Key with hash ${hash} revoked.`);
      await onRefresh();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="stack">
      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      {success && (
        <div className="card" style={{ borderLeft: "4px solid var(--good)" }}>
          <p style={{ margin: 0 }}>{success}</p>
        </div>
      )}

      {/* Add New Key Form */}
      <div className="card stack">
        <h3>Add Operator API Key</h3>
        <p className="muted small">
          Keys authenticate external tooling and operators against the control plane.
          Secret material is hashed upon receipt and never stored in plaintext or echoed back.
        </p>

        <form onSubmit={(e) => void handleAddKey(e)} className="stack">
          <div className="grid-3">
            <div className="form-group">
              <label htmlFor="key-label">Key Label</label>
              <input
                id="key-label"
                type="text"
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                placeholder="ci-deployer / monitoring"
                required
                disabled={busy}
              />
            </div>
            <div className="form-group">
              <label htmlFor="key-role">Role</label>
              <select
                id="key-role"
                value={role}
                onChange={(e) => setRole(e.target.value)}
                disabled={busy}
              >
                <option value="readonly">Readonly (read status/telemetry only)</option>
                <option value="operator">Operator (standard mutations &amp; controls)</option>
                <option value="owner">Owner (administrative &amp; live promotions)</option>
              </select>
            </div>
            <div className="form-group">
              <label htmlFor="key-secret">Key Secret (min 24 characters)</label>
              <input
                id="key-secret"
                type="password"
                autoComplete="new-password"
                value={secretKey}
                onChange={(e) => setSecretKey(e.target.value)}
                placeholder="••••••••••••••••••••••••"
                required
                disabled={busy}
              />
            </div>
          </div>

          <div>
            <button
              type="submit"
              className="primary"
              disabled={busy || !label.trim() || secretKey.length < 24}
            >
              {busy ? "Registering…" : "Register Key"}
            </button>
          </div>
        </form>
      </div>

      {/* Existing Keys Table */}
      <div className="card" style={{ padding: 0, overflowX: "auto" }}>
        <table className="table" style={{ width: "100%", margin: 0 }}>
          <thead>
            <tr>
              <th>Label</th>
              <th>Role</th>
              <th>Key Hash / Fingerprint</th>
              <th>Source</th>
              <th>Status</th>
              <th>Created</th>
              <th style={{ textAlign: "right" }}>Action</th>
            </tr>
          </thead>
          <tbody>
            {keys.length === 0 ? (
              <tr>
                <td colSpan={7} className="muted" style={{ textAlign: "center", padding: "2rem" }}>
                  No operator keys found.
                </td>
              </tr>
            ) : (
              keys.map((k, idx) => {
                const hashValue = k.hash || k.key_hash || `key-${idx}`;
                return (
                  <tr key={hashValue}>
                    <td><strong>{k.label}</strong></td>
                    <td>
                      <span className={`tag ${k.role === "owner" ? "tag--active" : "tag--paper"}`}>
                        {k.role}
                      </span>
                    </td>
                    <td>
                      <code className="small">{hashValue.slice(0, 16)}…</code>
                    </td>
                    <td>
                      <span className="muted small">{k.source || "runtime"}</span>
                    </td>
                    <td>
                      <span className={`tag ${k.enabled !== false ? "tag--active" : "tag--danger"}`}>
                        {k.enabled !== false ? "Active" : "Revoked"}
                      </span>
                    </td>
                    <td className="small muted">
                      {k.created_at ? new Date(k.created_at).toLocaleDateString() : "—"}
                    </td>
                    <td style={{ textAlign: "right" }}>
                      {k.enabled !== false && (
                        <button
                          type="button"
                          className="danger"
                          disabled={busy}
                          onClick={() => void handleRevoke(hashValue)}
                        >
                          Revoke
                        </button>
                      )}
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

export default KeyList;
