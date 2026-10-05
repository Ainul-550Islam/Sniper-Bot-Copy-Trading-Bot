"use client";

import { MarketTicker } from "@/lib/api/market-api";

interface MarketCardProps {
  market: MarketTicker;
}

export default function MarketCard({ market }: MarketCardProps) {
  const isUp = market.change_24h_pct >= 0;

  return (
    <div className="card" style={{ marginBottom: "1rem" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: "0.5rem" }}>
        <div>
          <h3 style={{ margin: 0 }}>{market.symbol}</h3>
          <span style={{ fontSize: "0.8rem", color: "var(--muted)" }}>{market.name}</span>
        </div>
        <span className="badge" style={{ background: "rgba(255,255,255,0.08)" }}>
          {market.venue.toUpperCase()}
        </span>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "0.5rem", marginTop: "1rem" }}>
        <div>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Price</div>
          <div style={{ fontSize: "1.1rem", fontWeight: 600, fontFamily: "var(--mono)" }}>
            ${market.price_usd < 0.01 ? market.price_usd.toFixed(6) : market.price_usd.toFixed(2)}
          </div>
        </div>

        <div>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>24h Change</div>
          <div style={{ fontSize: "1.1rem", fontWeight: 600, color: isUp ? "var(--ok)" : "var(--bad)" }}>
            {isUp ? "+" : ""}{market.change_24h_pct.toFixed(2)}%
          </div>
        </div>

        <div>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>24h Volume</div>
          <div style={{ fontSize: "0.9rem" }}>${market.volume_24h_usd.toLocaleString()}</div>
        </div>

        <div>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Liquidity Depth</div>
          <div style={{ fontSize: "0.9rem" }}>${market.liquidity_usd.toLocaleString()}</div>
        </div>
      </div>
    </div>
  );
}
