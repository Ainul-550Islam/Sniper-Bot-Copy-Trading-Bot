"use client";

/**
 * Tenant Strategy Management Console (SECOND.md §51).
 *
 * Provides customer-facing strategy configuration, parameter tuning,
 * activation, pause, and version tracking for Sniper, Copy Trading,
 * and Polymarket CLOB modules.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import StrategyCard from "@/components/strategy/strategy-card";
import StrategyForm from "@/components/strategy/strategy-form";
import { StrategyRecord, listStrategies } from "@/lib/api/strategy-api";

export default function StrategiesPage() {
  const [strategies, setStrategies] = useState<StrategyRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [filterModule, setFilterModule] = useState<string>("all");

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const mod = filterModule === "all" ? undefined : filterModule;
      const data = await listStrategies(mod);
      setStrategies(data);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load strategies");
    } finally {
      setLoading(false);
    }
  }, [filterModule]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <AppShell title="Trading Strategies">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.5rem" }}>
        <div>
          <h1 style={{ margin: 0 }}>Automated Trading Strategies</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            Configure and govern deterministic execution logic across Solana DEXes and Polymarket CLOB.
          </p>
        </div>
        <button
          onClick={() => setShowCreate(!showCreate)}
          className="btn btn-primary"
        >
          {showCreate ? "Close Form" : "+ New Strategy"}
        </button>
      </div>

      {showCreate && (
        <StrategyForm
          onSuccess={() => {
            setShowCreate(false);
            void load();
          }}
          onCancel={() => setShowCreate(false)}
        />
      )}

      <div style={{ display: "flex", gap: "1rem", marginBottom: "1.5rem" }}>
        <button
          onClick={() => setFilterModule("all")}
          className={`btn ${filterModule === "all" ? "btn-primary" : "btn-secondary"}`}
          style={{ fontSize: "0.85rem" }}
        >
          All Modules ({strategies.length})
        </button>
        <button
          onClick={() => setFilterModule("sniper")}
          className={`btn ${filterModule === "sniper" ? "btn-primary" : "btn-secondary"}`}
          style={{ fontSize: "0.85rem" }}
        >
          Solana Sniper
        </button>
        <button
          onClick={() => setFilterModule("copy")}
          className={`btn ${filterModule === "copy" ? "btn-primary" : "btn-secondary"}`}
          style={{ fontSize: "0.85rem" }}
        >
          Copy Trading
        </button>
        <button
          onClick={() => setFilterModule("polymarket")}
          className={`btn ${filterModule === "polymarket" ? "btn-primary" : "btn-secondary"}`}
          style={{ fontSize: "0.85rem" }}
        >
          Polymarket CLOB
        </button>
      </div>

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          {error}
        </div>
      )}

      {loading ? (
        <div className="card">Loading strategy catalog...</div>
      ) : strategies.length === 0 ? (
        <div className="card" style={{ textAlign: "center", padding: "3rem" }}>
          <h3 style={{ margin: 0 }}>No Strategies Configured</h3>
          <p style={{ color: "var(--muted)", margin: "0.5rem 0 1.5rem" }}>
            Create your first automated execution strategy to start sniping tokens or copying profitable traders.
          </p>
          <button onClick={() => setShowCreate(true)} className="btn btn-primary">
            Create Strategy
          </button>
        </div>
      ) : (
        <div>
          {strategies.map((strategy) => (
            <StrategyCard key={strategy.id} strategy={strategy} onRefresh={() => void load()} />
          ))}
        </div>
      )}
    </AppShell>
  );
}
