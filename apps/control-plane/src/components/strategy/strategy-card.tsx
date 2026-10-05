"use client";

import { useState } from "react";
import { StrategyRecord, archiveStrategy, updateStrategy } from "@/lib/api/strategy-api";

interface StrategyCardProps {
  strategy: StrategyRecord;
  onRefresh: () => void;
}

export default function StrategyCard({ strategy, onRefresh }: StrategyCardProps) {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const toggleStatus = async () => {
    setLoading(true);
    setError(null);
    try {
      const nextStatus = strategy.status === "active" ? "paused" : "active";
      await updateStrategy(strategy.id, { status: nextStatus });
      onRefresh();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to toggle status");
    } finally {
      setLoading(false);
    }
  };

  const handleArchive = async () => {
    if (!confirm(`Are you sure you want to archive strategy "${strategy.name}"?`)) return;
    setLoading(true);
    setError(null);
    try {
      await archiveStrategy(strategy.id);
      onRefresh();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to archive strategy");
    } finally {
      setLoading(false);
    }
  };

  const statusBadgeClass =
    strategy.status === "active"
      ? "badge badge-ok"
      : strategy.status === "paused"
      ? "badge badge-warn"
      : strategy.status === "archived"
      ? "badge badge-bad"
      : "badge";

  return (
    <div className="card" style={{ marginBottom: "1rem" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: "0.75rem" }}>
        <div>
          <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
            <h3 style={{ margin: 0 }}>{strategy.name}</h3>
            <span className={statusBadgeClass}>{strategy.status.toUpperCase()}</span>
            <span className="badge" style={{ background: "rgba(255,255,255,0.08)" }}>
              v{strategy.version}
            </span>
            <span className="badge" style={{ background: "var(--accent-glow)", color: "var(--accent)" }}>
              {strategy.module_family.toUpperCase()}
            </span>
          </div>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.85rem" }}>
            {strategy.description || "No description provided."}
          </p>
        </div>
        <div style={{ display: "flex", gap: "0.5rem" }}>
          {strategy.status !== "archived" && (
            <button
              onClick={toggleStatus}
              disabled={loading}
              className={`btn ${strategy.status === "active" ? "btn-secondary" : "btn-primary"}`}
              style={{ fontSize: "0.8rem", padding: "0.3rem 0.6rem" }}
            >
              {strategy.status === "active" ? "Pause" : "Activate"}
            </button>
          )}
          {strategy.status !== "archived" && (
            <button
              onClick={handleArchive}
              disabled={loading}
              className="btn btn-secondary"
              style={{ fontSize: "0.8rem", padding: "0.3rem 0.6rem", color: "var(--bad)" }}
            >
              Archive
            </button>
          )}
        </div>
      </div>

      {error && (
        <div style={{ color: "var(--bad)", fontSize: "0.85rem", marginBottom: "0.5rem" }}>
          {error}
        </div>
      )}

      <div style={{ background: "rgba(0,0,0,0.2)", padding: "0.75rem", borderRadius: "6px" }}>
        <h4 style={{ margin: "0 0 0.5rem", fontSize: "0.8rem", color: "var(--muted)" }}>Configured Parameters</h4>
        <pre style={{ margin: 0, fontSize: "0.75rem", fontFamily: "var(--mono)", overflowX: "auto" }}>
          {JSON.stringify(strategy.parameters, null, 2)}
        </pre>
      </div>

      <div style={{ marginTop: "0.75rem", display: "flex", justifyContent: "space-between", fontSize: "0.75rem", color: "var(--muted)" }}>
        <span>ID: <code>{strategy.id}</code></span>
        <span>Updated: {new Date(strategy.updated_at).toLocaleString()}</span>
      </div>
    </div>
  );
}
