"use client";

/**
 * Tenant API key management.
 *
 * This page deliberately has no demo data and no client-side secret fallback:
 * a failed read is shown as an error, and the one-time secret is rendered only
 * when the server returns it after a successful creation.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { apiKeys, toDisplayError, type ApiKeyMetadata } from "@/lib/api";

export default function ApiKeysSettingsPage() {
  const [keys, setKeys] = useState<ApiKeyMetadata[]>([]);
  const [label, setLabel] = useState("");
  const [role, setRole] = useState("trader");
  const [loading, setLoading] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [revokingPrefix, setRevokingPrefix] = useState<string | null>(null);
  const [newSecret, setNewSecret] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadKeys = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const response = await apiKeys.list();
      setKeys(response.keys ?? []);
    } catch (err: unknown) {
      setKeys([]);
      setError(toDisplayError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadKeys();
  }, [loadKeys]);

  const handleCreate = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const trimmedLabel = label.trim();
    if (!trimmedLabel) {
      setError("A key label is required.");
      return;
    }

    setSubmitting(true);
    setError(null);
    try {
      const response = await apiKeys.create(trimmedLabel, role);
      setNewSecret(response.secret);
      setLabel("");
      await loadKeys();
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setSubmitting(false);
    }
  };

  const handleRevoke = async (prefix: string) => {
    if (!window.confirm(`Revoke API key ${prefix}? Any automated scripts using this key will be disconnected.`)) {
      return;
    }

    setRevokingPrefix(prefix);
    setError(null);
    try {
      await apiKeys.revoke(prefix);
      await loadKeys();
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setRevokingPrefix(null);
    }
  };

  return (
    <AppShell title="API Keys">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Tenant API Keys</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Generate scoped credentials for tenant trading and backtest data-plane access. Secrets are returned once and are never recoverable.
        </p>
      </div>

      {newSecret && (
        <div
          className="card"
          style={{
            borderColor: "var(--ok)",
            background: "var(--ok-glow)",
            marginBottom: "1.5rem",
          }}
        >
          <h3 style={{ marginTop: 0, color: "var(--ok)" }}>API Key Generated Successfully</h3>
          <p style={{ fontSize: "0.85rem", margin: "0.25rem 0 0.5rem" }}>
            Copy this secret token now. For security reasons, it will never be displayed again.
          </p>
          <code
            style={{
              fontSize: "0.95rem",
              padding: "0.5rem",
              display: "block",
              background: "rgba(0,0,0,0.5)",
              overflowWrap: "anywhere",
            }}
          >
            {newSecret}
          </code>
          <button
            type="button"
            onClick={() => setNewSecret(null)}
            className="btn btn-secondary"
            style={{ marginTop: "0.75rem", fontSize: "0.8rem" }}
          >
            I have securely saved this key
          </button>
        </div>
      )}

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          <div>{error}</div>
          {error.startsWith("http_401:") || error.startsWith("http_403:") ? (
            <button type="button" onClick={() => void loadKeys()} className="btn btn-secondary" style={{ marginTop: "0.75rem" }}>
              Retry
            </button>
          ) : null}
        </div>
      )}

      <div className="card" style={{ marginBottom: "1.5rem" }}>
        <h3 style={{ marginTop: 0 }}>Generate New API Key</h3>
        <form onSubmit={handleCreate} style={{ display: "flex", gap: "1rem", alignItems: "flex-end", flexWrap: "wrap" }}>
          <div style={{ flex: 2, minWidth: "220px" }}>
            <label htmlFor="api-key-label" style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Key Label / Application Name
            </label>
            <input
              id="api-key-label"
              type="text"
              required
              maxLength={128}
              value={label}
              onChange={(event) => setLabel(event.target.value)}
              placeholder="e.g. Backtest Analytics Daemon"
              className="input"
              style={{ width: "100%" }}
            />
          </div>

          <div style={{ flex: 1, minWidth: "140px" }}>
            <label htmlFor="api-key-role" style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Scope &amp; Role
            </label>
            <select
              id="api-key-role"
              value={role}
              onChange={(event) => setRole(event.target.value)}
              className="select"
              style={{ width: "100%" }}
            >
              <option value="trader">Trader (Read &amp; Execute)</option>
              <option value="viewer">Viewer (Read Only)</option>
              <option value="auditor">Auditor (Audit Read)</option>
            </select>
          </div>

          <button type="submit" disabled={submitting} className="btn btn-primary" style={{ height: "38px" }}>
            {submitting ? "Generating..." : "Generate Key"}
          </button>
        </form>
      </div>

      <div className="card" style={{ padding: 0, overflow: "hidden" }}>
        <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
          <h3 style={{ margin: 0 }}>API Keys ({keys.length})</h3>
        </div>

        {loading ? (
          <div style={{ padding: "2rem", textAlign: "center" }}>Loading API keys...</div>
        ) : keys.length === 0 ? (
          <div style={{ padding: "2rem", textAlign: "center", color: "var(--muted)" }}>
            No API keys are configured for this organization.
          </div>
        ) : (
          <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
            <thead>
              <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                <th style={{ padding: "0.75rem 1rem" }}>Key Label</th>
                <th style={{ padding: "0.75rem 1rem" }}>Prefix</th>
                <th style={{ padding: "0.75rem 1rem" }}>Role</th>
                <th style={{ padding: "0.75rem 1rem" }}>Status</th>
                <th style={{ padding: "0.75rem 1rem" }}>Created</th>
                <th style={{ padding: "0.75rem 1rem" }}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {keys.map((key) => (
                <tr key={key.id} style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <strong>{key.label}</strong>
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <code>{key.key_prefix}…</code>
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <span className="badge">{key.role.toUpperCase()}</span>
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <span className={`badge ${key.usable ? "badge-ok" : "badge-bad"}`}>
                      {key.usable ? "ACTIVE" : key.revoked_at ? "REVOKED" : "EXPIRED"}
                    </span>
                  </td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
                    {new Date(key.created_at).toLocaleDateString()}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    {key.usable ? (
                      <button
                        type="button"
                        onClick={() => void handleRevoke(key.key_prefix)}
                        disabled={revokingPrefix === key.key_prefix}
                        className="btn btn-secondary"
                        style={{ fontSize: "0.75rem", padding: "0.2rem 0.5rem", color: "var(--bad)" }}
                      >
                        {revokingPrefix === key.key_prefix ? "Revoking..." : "Revoke"}
                      </button>
                    ) : (
                      <span style={{ color: "var(--muted)", fontSize: "0.8rem" }}>Unavailable</span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </AppShell>
  );
}
