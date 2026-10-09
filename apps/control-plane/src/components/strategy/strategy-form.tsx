"use client";

import { useState } from "react";
import { CreateStrategyInput, StrategyModule, createStrategy } from "@/lib/api/strategy-api";

interface StrategyFormProps {
  onSuccess: () => void;
  onCancel: () => void;
}

export default function StrategyForm({ onSuccess, onCancel }: StrategyFormProps) {
  const [moduleFamily, setModuleFamily] = useState<StrategyModule>("sniper");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [rawParams, setRawParams] = useState(
    JSON.stringify(
      {
        target_tokens: ["So11111111111111111111111111111111111111112"],
        max_buy_sol: 1.5,
        slippage_bps: 100,
        anti_rug_min_liquidity_usd: 10000,
        auto_sell_take_profit_pct: 50,
        auto_sell_stop_loss_pct: 15,
        mev_protection_tip_lamports: 100000,
      },
      null,
      2,
    ),
  );
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleModuleChange = (mod: StrategyModule) => {
    setModuleFamily(mod);
    if (mod === "sniper") {
      setRawParams(
        JSON.stringify(
          {
            target_tokens: ["So11111111111111111111111111111111111111112"],
            max_buy_sol: 1.5,
            slippage_bps: 100,
            anti_rug_min_liquidity_usd: 10000,
            auto_sell_take_profit_pct: 50,
            auto_sell_stop_loss_pct: 15,
            mev_protection_tip_lamports: 100000,
          },
          null,
          2,
        ),
      );
    } else if (mod === "copy") {
      setRawParams(
        JSON.stringify(
          {
            lead_wallets: ["9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"],
            copy_ratio_pct: 50,
            max_trade_sol: 2.0,
            follow_sells: true,
            min_lead_balance_sol: 10.0,
          },
          null,
          2,
        ),
      );
    } else if (mod === "polymarket") {
      setRawParams(
        JSON.stringify(
          {
            market_slugs: ["presidential-election-winner-2024"],
            max_position_usd: 5000,
            spread_threshold_bps: 30,
            clob_order_type: "FAK",
          },
          null,
          2,
        ),
      );
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setError(null);

    try {
      let parsedParams: Record<string, unknown>;
      try {
        parsedParams = JSON.parse(rawParams);
      } catch {
        throw new Error("Parameters must be valid JSON object");
      }

      const payload: CreateStrategyInput = {
        module_family: moduleFamily,
        name,
        description,
        parameters: parsedParams,
      };

      await createStrategy(payload);
      onSuccess();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to create strategy");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="card" style={{ marginBottom: "1.5rem" }}>
      <h3 style={{ marginTop: 0 }}>Create New Strategy</h3>
      {error && (
        <div style={{ color: "var(--bad)", background: "var(--bad-glow)", padding: "0.5rem", borderRadius: "4px", marginBottom: "1rem" }}>
          {error}
        </div>
      )}

      <form onSubmit={handleSubmit}>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 2fr", gap: "1rem", marginBottom: "1rem" }}>
          <div>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Trading Module
            </label>
            <select
              value={moduleFamily}
              onChange={(e) => handleModuleChange(e.target.value as StrategyModule)}
              className="select"
              style={{ width: "100%" }}
            >
              <option value="sniper">Solana Sniper</option>
              <option value="copy">Solana Copy Trading</option>
              <option value="polymarket">Polymarket CLOB</option>
            </select>
          </div>

          <div>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Strategy Name
            </label>
            <input
              type="text"
              required
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. Raydium Launch Sniper"
              className="input"
              style={{ width: "100%" }}
            />
          </div>
        </div>

        <div style={{ marginBottom: "1rem" }}>
          <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
            Description
          </label>
          <input
            type="text"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Execution parameters and anti-rug safeguards"
            className="input"
            style={{ width: "100%" }}
          />
        </div>

        <div style={{ marginBottom: "1rem" }}>
          <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
            Strategy Parameters (JSON)
          </label>
          <textarea
            rows={7}
            value={rawParams}
            onChange={(e) => setRawParams(e.target.value)}
            className="input"
            style={{ width: "100%", fontFamily: "var(--mono)", fontSize: "0.8rem" }}
          />
        </div>

        <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.5rem" }}>
          <button type="button" onClick={onCancel} className="btn btn-secondary">
            Cancel
          </button>
          <button type="submit" disabled={loading} className="btn btn-primary">
            {loading ? "Creating..." : "Save Strategy"}
          </button>
        </div>
      </form>
    </div>
  );
}
