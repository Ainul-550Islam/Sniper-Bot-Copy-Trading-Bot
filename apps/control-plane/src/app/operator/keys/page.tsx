"use client";

import { useEffect, useState, useCallback } from "react";
import { KeyList } from "@/components/operator/KeyList";
import { getKeys, type OperatorKey } from "@/lib/api/operator-api";
import {
  operatorKeyHash,
  operatorSessionStore,
} from "@/lib/operator-session";

/**
 * Operator API keys workspace (`/operator/keys`).
 *
 * Backed by `GET|POST /api/keys` and `DELETE /api/keys/:hash`, all three of
 * which are owner-role operator routes: they need the operator key, which the
 * surrounding `OperatorGuard` has already collected.
 *
 * The page never sees plaintext key material. `addKey` sends the typed secret
 * once (it is hashed server-side), the list shows only SHA-256 hashes, and
 * revoking the key that is currently unlocking this console demands a longer
 * typed phrase than any other key.
 */
export default function OperatorKeysPage() {
  const [keys, setKeys] = useState<OperatorKey[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [protectedHash, setProtectedHash] = useState<string | null>(null);

  const loadKeys = useCallback(async () => {
    try {
      const response = await getKeys();
      setKeys(response.keys ?? []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshProtectedHash = useCallback(async () => {
    setProtectedHash(await operatorKeyHash());
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadKeys());
  }, [loadKeys]);

  // Recompute whenever the console is unlocked with a different key, so the
  // "in use" marker tracks the key actually being presented.
  useEffect(() => {
    void Promise.resolve().then(() => refreshProtectedHash());
    return operatorSessionStore.subscribe(() => {
      void refreshProtectedHash();
    });
  }, [refreshProtectedHash]);

  return (
    <article className="stack">
      <div className="row-between">
        <div>
          <h1>Operator API Keys (/api/keys)</h1>
          <p className="muted">
            The operator key registry. Every operator route is authenticated by one of
            these keys presented as <code>x-api-key</code>; the role that authorizes a
            call is the role bundled with the key, not a tenant membership.
          </p>
        </div>
        <div>
          <button type="button" onClick={() => void loadKeys()} disabled={loading}>
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      <KeyList keys={keys} onRefresh={loadKeys} protectedHash={protectedHash} />
    </article>
  );
}
