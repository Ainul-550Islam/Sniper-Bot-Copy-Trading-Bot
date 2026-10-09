"use client";

import { MarketTicker, changePct, priceUsd, venueLabel } from "@/lib/api/market-api";
import { formatUsdCents } from "@/lib/formatters/financial";

interface MarketCardProps {
  market: MarketTicker;
}

export default function MarketCard({ market }: MarketCardProps) {
  const pct = changePct(market);
  const isUp = pct >= 0;
  const price = priceUsd(market);

  return (
    <div className="card" style={{ marginBottom: "1rem" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: "0.5rem" }}>
        <div>
          <h3 style={{ margin: 0 }}>{market.symbol}</h3>
          <span style={{ fontSize: "0.8rem", color: "var(--muted)" }}>{market.name}</span>
        </div>
        <span className="badge" style={{ background: "rgba(255,255,255,0.08)" }}>
          {venueLabel(market.venue)}
        </span>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "0.5rem", marginTop: "1rem" }}>
        <div>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Price</div>
          <div style={{ fontSize: "1.1rem", fontWeight: 600, fontFamily: "var(--mono)" }}>
            ${price < 0.01 ? price.toFixed(6) : price.toFixed(2)}
          </div>
        </div>

        <div>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>24h Change</div>
          <div style={{ fontSize: "1.1rem", fontWeight: 600, color: isUp ? "var(--ok)" : "var(--bad)" }}>
            {isUp ? "+" : ""}{pct.toFixed(2)}%
          </div>
        </div>

        <div>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>24h Volume</div>
          <div style={{ fontSize: "0.9rem" }}>{formatUsdCents(market.volume_24h_usd_cents)}</div>
        </div>

        <div>
          <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Liquidity Depth</div>
          <div style={{ fontSize: "0.9rem" }}>{formatUsdCents(market.liquidity_usd_cents)}</div>
        </div>
      </div>
    </div>
  );
}
