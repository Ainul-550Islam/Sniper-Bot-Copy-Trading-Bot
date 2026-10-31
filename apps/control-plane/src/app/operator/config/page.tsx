"use client";

import { useEffect, useState, useCallback } from "react";
import { getConfig } from "@/lib/api/operator-api";

function containsUnredactedSecret(obj: unknown): boolean {
  if (!obj || typeof obj !== "object") return false;

  for (const [key, value] of Object.entries(obj as Record<string, unknown>)) {
    const lowerKey = key.toLowerCase();
    const isSensitiveName = [
      "secret",
      "password",
      "token",
      "api_key",
      "private_key",
      "signing_key",
    ].some((s) => lowerKey.includes(s));

    if (isSensitiveName && typeof value === "string") {
      if (value !== "<redacted>" && value.length > 0) {
        return true;
      }
    }

    if (typeof value === "object" && value !== null) {
      if (containsUnredactedSecret(value)) return true;
    }
  }

  return false;
}

export default function OperatorConfigPage() {
  const [config, setConfig] = useState<Record<string, unknown> | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [hasLeakViolation, setHasLeakViolation] = useState(false);

  const loadConfig = useCallback(async () => {
    try {
      const data = await getConfig();
      const leaked = containsUnredactedSecret(data);
      setHasLeakViolation(leaked);
      setConfig(data);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadConfig());
  }, [loadConfig]);

  return (
    <article className="stack">
      <div className="row-between">
        <div>
          <h1>Runtime Configuration (/api/config)</h1>
          <p className="muted">
            Active server configuration snapshot. The backend enforces recursive redaction of all sensitive keys prior to serialization.
          </p>
        </div>
        <div>
          <button type="button" onClick={() => void loadConfig()} disabled={loading}>
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      {hasLeakViolation && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <h4 style={{ color: "var(--bad)", margin: 0 }}>WARNING: Potential Unredacted Secret Detected</h4>
          <p className="small muted" style={{ margin: "0.3rem 0 0" }}>
            The UI assertion flagged a field with sensitive naming that does not equal <code>&lt;redacted&gt;</code>.
          </p>
        </div>
      )}

      <div className="card">
        <div className="row-between" style={{ marginBottom: "1rem" }}>
          <h4>Configuration Snapshot</h4>
          <span className="tag tag--active">Redacted Read-Only View</span>
        </div>
        <pre
          style={{
            background: "var(--panel-2)",
            padding: "1rem",
            borderRadius: "4px",
            overflowX: "auto",
            maxHeight: "600px",
            fontSize: "0.85rem",
          }}
        >
          {config ? JSON.stringify(config, null, 2) : loading ? "Loading configuration…" : "Configuration unavailable."}
        </pre>
      </div>
    </article>
  );
}
