"use client";

import { useState } from "react";
import { TypedConfirm } from "@/components/lifecycle/TypedConfirm";
import {
  addKey,
  revokeKey,
  type OperatorKey,
} from "@/lib/api/operator-api";

function getErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}

/** Stable display id for a key row: the backend's SHA-256 hash. */
function hashOf(key: OperatorKey, index: number): string {
  return key.hash || key.key_hash || `key-${index}`;
}

export interface KeyListProps {
  keys: OperatorKey[];
  onRefresh: () => Promise<void> | void;
  /**
   * SHA-256 hex of the operator key currently unlocking this console, when it
   * can be computed (see `operatorKeyHash`). Revoking that key locks the
   * operator out of every route, so it needs a stronger typed phrase than any
   * other key.
   */
  protectedHash?: string | null;
}

export function KeyList({ keys, onRefresh, protectedHash = null }: KeyListProps) {
  const [label, setLabel] = useState("");
  const [role, setRole] = useState("operator");
  const [secretKey, setSecretKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const [revokeTarget, setRevokeTarget] = useState<{ hash: string; label: string } | null>(null);

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

      // Clear the secret field immediately after submit — on success AND on
      // failure. The plaintext exists only for the lifetime of this call and is
      // never rendered back to the operator.
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
    setBusy(true);
    setError(null);
    try {
      await revokeKey(hash);
      setRevokeTarget(null);
      setSuccess(`Key with hash ${hash.slice(0, 16)}… revoked.`);
      await onRefresh();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const target = revokeTarget;
  const targetIsInUse = target !== null && target.hash === protectedHash;

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
                const hashValue = hashOf(k, idx);
                const inUse = hashValue === protectedHash;
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
                      {inUse ? (
                        <span className="tag tag--active">In use</span>
                      ) : (
                        <span className={`tag ${k.enabled !== false ? "tag--active" : "tag--danger"}`}>
                          {k.enabled !== false ? "Active" : "Revoked"}
                        </span>
                      )}
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
                          onClick={() => setRevokeTarget({ hash: hashValue, label: k.label })}
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
        <p className="muted small" style={{ padding: "0.75rem 1rem", margin: 0 }}>
          Only the key hash is ever listed or returned by the API. Plaintext key material
          exists solely for the lifetime of the &ldquo;Register Key&rdquo; request and is
          never displayed, logged or retained.
        </p>
      </div>

      {target && (
        <div className="card">
          <TypedConfirm
            title={targetIsInUse ? "Revoke the key in use" : "Revoke operator key"}
            description={
              targetIsInUse
                ? `This is the key currently unlocking this console. Revoking it returns 401 on every operator route immediately and locks the console until another owner key is entered. Label: ${target.label} · hash ${target.hash.slice(0, 16)}…`
                : `Permanently revoke '${target.label}' (hash ${target.hash.slice(0, 16)}…). Every client presenting this key is refused from the next request onward.`
            }
            expected={
              targetIsInUse
                ? `REVOKE MY KEY ${target.label}`
                : `REVOKE ${target.label}`
            }
            confirmLabel={targetIsInUse ? "Revoke the key in use" : "Revoke key"}
            busy={busy}
            onConfirm={() => void handleRevoke(target.hash)}
          />
          <div style={{ marginTop: "0.75rem" }}>
            <button type="button" disabled={busy} onClick={() => setRevokeTarget(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

export default KeyList;
