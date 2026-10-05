"use client";

/**
 * Sniper Strategy Configuration & Parameter Tuning (SECOND.md §54).
 *
 * Dedicated control console for Solana token sniping, MEV bundle tipping,
 * anti-rug liquidity requirements, and automated take-profit / stop-loss.
 */

import { useState } from "react";
import AppShell from "@/components/AppShell";
import { createStrategy } from "@/lib/api/strategy-api";

export default function SniperConfigPage() {
  const [name, setName] = useState("");
  const [maxBuySol, setMaxBuySol] = useState<number | "">("");
  const [slippageBps, setSlippageBps] = useState<number | "">("");
  const [minLiquidityUsd, setMinLiquidityUsd] = useState<number | "">("");
  const [takeProfitPct, setTakeProfitPct] = useState<number | "">("");
  const [stopLossPct, setStopLossPct] = useState<number | "">("");
  const [tipLamports, setTipLamports] = useState<number | "">("");
  const [loading, setLoading] = useState(false);
  const [status, setStatus] = useState<{ message: string; ok: boolean } | null>(null);

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setStatus(null);

    try {
      await createStrategy({
        module_family: "sniper",
        name,
        description: "Autonomous Solana DEX sniper with MEV bundle execution",
        parameters: {
          target_tokens: ["So11111111111111111111111111111111111111112"],
          max_buy_sol: maxBuySol,
          slippage_bps: slippageBps,
          anti_rug_min_liquidity_usd: minLiquidityUsd,
          auto_sell_take_profit_pct: takeProfitPct,
          auto_sell_stop_loss_pct: stopLossPct,
          mev_protection_tip_lamports: tipLamports,
        },
      });
      setStatus({ message: "Sniper strategy saved to the authenticated tenant.", ok: true });
    } catch (err: unknown) {
      setStatus({ message: err instanceof Error ? err.message : "Failed to save sniper configuration", ok: false });
    } finally {
      setLoading(false);
    }
  };

  return (
    <AppShell title="Sniper Configuration">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Solana Sniper Strategy Configuration</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Tune block-0 latency, anti-rug heuristics, Jito MEV tip amounts, and automated sell rules.
        </p>
      </div>

      {status && (
        <div
          className="card"
          style={{
            color: status.ok ? "var(--ok)" : "var(--bad)",
            background: status.ok ? "var(--ok-glow)" : "var(--bad-glow)",
            marginBottom: "1.5rem",
          }}
        >
          {status.message}
        </div>
      )}

      <div className="card">
        <form onSubmit={handleSave}>
          <div style={{ marginBottom: "1.5rem" }}>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Strategy Profile Name
            </label>
            <input
              type="text"
              required
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="input"
              style={{ width: "100%", maxWidth: "480px" }}
            />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "1.5rem", marginBottom: "1.5rem" }}>
            <div>
              <h3 style={{ fontSize: "1rem", marginTop: 0 }}>Capital &amp; Execution</h3>
              <div style={{ marginBottom: "1rem" }}>
                <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                  Max Buy Amount (SOL)
                </label>
                <input
                  type="number"
                  step="0.1"
                  min="0.1"
                  value={maxBuySol}
                  onChange={(e) => setMaxBuySol(Number(e.target.value))}
                  className="input"
                  style={{ width: "100%" }}
                />
              </div>

              <div style={{ marginBottom: "1rem" }}>
                <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                  Max Slippage Tolerance (BPS)
                </label>
                <input
                  type="number"
                  min="10"
                  max="2500"
                  value={slippageBps}
                  onChange={(e) => setSlippageBps(Number(e.target.value))}
                  className="input"
                  style={{ width: "100%" }}
                />
                <span style={{ fontSize: "0.75rem", color: "var(--muted)" }}>100 BPS = 1.0%</span>
              </div>

              <div>
                <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                  Jito MEV Tip (Lamports)
                </label>
                <input
                  type="number"
                  step="10000"
                  min="10000"
                  value={tipLamports}
                  onChange={(e) => setTipLamports(Number(e.target.value))}
                  className="input"
                  style={{ width: "100%" }}
                />
                <span style={{ fontSize: "0.75rem", color: "var(--muted)" }}>500,000 lamports = 0.0005 SOL</span>
              </div>
            </div>

            <div>
              <h3 style={{ fontSize: "1rem", marginTop: 0 }}>Safeguards &amp; Automated Exits</h3>
              <div style={{ marginBottom: "1rem" }}>
                <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                  Anti-Rug Min Pool Liquidity (USD)
                </label>
                <input
                  type="number"
                  step="1000"
                  min="1000"
                  value={minLiquidityUsd}
                  onChange={(e) => setMinLiquidityUsd(Number(e.target.value))}
                  className="input"
                  style={{ width: "100%" }}
                />
              </div>

              <div style={{ marginBottom: "1rem" }}>
                <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                  Auto Take-Profit Target (%)
                </label>
                <input
                  type="number"
                  min="5"
                  value={takeProfitPct}
                  onChange={(e) => setTakeProfitPct(Number(e.target.value))}
                  className="input"
                  style={{ width: "100%" }}
                />
              </div>

              <div>
                <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                  Auto Stop-Loss Limit (%)
                </label>
                <input
                  type="number"
                  min="1"
                  max="90"
                  value={stopLossPct}
                  onChange={(e) => setStopLossPct(Number(e.target.value))}
                  className="input"
                  style={{ width: "100%" }}
                />
              </div>
            </div>
          </div>

          <button type="submit" disabled={loading} className="btn btn-primary">
            {loading ? "Saving Configuration..." : "Save & Activate Sniper Configuration"}
          </button>
        </form>
      </div>
    </AppShell>
  );
}
