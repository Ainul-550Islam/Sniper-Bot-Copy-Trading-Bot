"use client";

import { PortfolioSummary } from "@/lib/api/portfolio-api";
import { formatBps, formatUsdCents } from "@/lib/formatters/financial";

interface PortfolioSummaryProps {
  portfolio: PortfolioSummary;
}

export function PortfolioSummaryCards({ portfolio }: PortfolioSummaryProps) {
  const isUnrealizedProfitable = portfolio.unrealized_pnl_usd_cents >= 0;
  const isRealizedProfitable = portfolio.realized_pnl_30d_usd_cents >= 0;

  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fill, minmax(240px, 1fr))",
        gap: "1rem",
        marginBottom: "1.5rem",
      }}
    >
      <div className="card">
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Total Organization Equity</div>
        <div style={{ fontSize: "1.6rem", fontWeight: 700, margin: "0.5rem 0 0.25rem", fontFamily: "var(--mono)" }}>
          {formatUsdCents(portfolio.total_equity_usd_cents)}
        </div>
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Authoritative multi-venue equity</div>
      </div>

      <div className="card">
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Available Cash &amp; Reserves</div>
        <div style={{ fontSize: "1.6rem", fontWeight: 700, margin: "0.5rem 0 0.25rem", fontFamily: "var(--mono)" }}>
          {formatUsdCents(portfolio.available_cash_usd_cents)}
        </div>
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>
          Margin Allocated: {formatUsdCents(portfolio.allocated_margin_usd_cents)}
        </div>
      </div>

      <div className="card">
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Unrealized Position PnL</div>
        <div
          style={{
            fontSize: "1.6rem",
            fontWeight: 700,
            margin: "0.5rem 0 0.25rem",
            fontFamily: "var(--mono)",
            color: isUnrealizedProfitable ? "var(--ok)" : "var(--bad)",
          }}
        >
          {formatUsdCents(portfolio.unrealized_pnl_usd_cents, { showSign: true })}
        </div>
        <div style={{ fontSize: "0.8rem", color: isRealizedProfitable ? "var(--ok)" : "var(--bad)" }}>
          30d Realized: {formatUsdCents(portfolio.realized_pnl_30d_usd_cents, { showSign: true })}
        </div>
      </div>

      <div className="card">
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Max Drawdown Peak</div>
        <div style={{ fontSize: "1.6rem", fontWeight: 700, margin: "0.5rem 0 0.25rem", fontFamily: "var(--mono)", color: "var(--bad)" }}>
          {formatBps(portfolio.max_drawdown_bps)}
        </div>
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Trailing 30-day max drawdown</div>
      </div>
    </div>
  );
}

export default PortfolioSummaryCards;
