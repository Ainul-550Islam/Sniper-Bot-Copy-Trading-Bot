"use client";

/**
 * Customer Portfolio Command Center (THIRD.md §101).
 *
 * Visualizes authoritative multi-venue equity, cash reserves, allocated margin,
 * unrealized/realized PnL, drawdown, and holding exposures.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import ExposureTable from "@/components/portfolio/ExposureTable";
import PortfolioSummaryCards from "@/components/portfolio/PortfolioSummary";
import { ErrorState } from "@/components/common/ErrorState";
import { PortfolioSummary, getPortfolioSummary } from "@/lib/api/portfolio-api";

export default function PortfolioPage() {
  const [portfolio, setPortfolio] = useState<PortfolioSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await getPortfolioSummary();
      setPortfolio(data);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load portfolio summary");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadData());
  }, [loadData]);

  return (
    <AppShell title="Portfolio Command Center">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.5rem" }}>
        <div>
          <h1 style={{ margin: 0 }}>Portfolio Command Center</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            Authoritative multi-venue equity, cash reserves, and asset exposure breakdown.
          </p>
        </div>
        <button onClick={() => void loadData()} className="btn btn-secondary" style={{ fontSize: "0.85rem" }}>
          Refresh Portfolio
        </button>
      </div>

      {error && <ErrorState error={error} onRetry={() => void loadData()} />}

      {loading ? (
        <div className="card">Loading portfolio projections...</div>
      ) : portfolio ? (
        <div>
          <PortfolioSummaryCards portfolio={portfolio} />
          <ExposureTable exposures={portfolio.exposures} />
        </div>
      ) : null}
    </AppShell>
  );
}
