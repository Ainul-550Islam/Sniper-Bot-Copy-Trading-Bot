"use client";

/**
 * Copy Trading Configuration Console (SECOND.md §55).
 *
 * Manage leader wallet subscriptions, mirror ratios, max trade limits,
 * and automated sell replication logic.
 */

import { useState } from "react";
import AppShell from "@/components/AppShell";
import { createStrategy } from "@/lib/api/strategy-api";

export default function CopyConfigPage() {
  const [name, setName] = useState("");
  const [leadWallets, setLeadWallets] = useState("");
  const [copyRatioPct, setCopyRatioPct] = useState<number | "">("");
  const [maxTradeSol, setMaxTradeSol] = useState<number | "">("");
  const [followSells, setFollowSells] = useState(false);
  const [minLeadBalanceSol, setMinLeadBalanceSol] = useState<number | "">("");
  const [loading, setLoading] = useState(false);
  const [status, setStatus] = useState<{ message: string; ok: boolean } | null>(null);

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setStatus(null);

    const wallets = leadWallets
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);

    try {
      await createStrategy({
        module_family: "copy",
        name,
        description: "Autonomous copy trading engine mirroring high-winrate on-chain alpha wallets",
        parameters: {
          lead_wallets: wallets,
          copy_ratio_pct: copyRatioPct,
          max_trade_sol: maxTradeSol,
          follow_sells: followSells,
          min_lead_balance_sol: minLeadBalanceSol,
        },
      });
      setStatus({ message: "Copy trading strategy saved to the authenticated tenant.", ok: true });
    } catch (err: unknown) {
      setStatus({ message: err instanceof Error ? err.message : "Failed to save configuration", ok: false });
    } finally {
      setLoading(false);
    }
  };

  return (
    <AppShell title="Copy Trading Configuration">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Copy Trading Engine Configuration</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Subscribe to high-volume alpha wallets and mirror swaps with deterministic position sizing.
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

          <div style={{ marginBottom: "1.5rem" }}>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Leader Wallet Addresses (One per line)
            </label>
            <textarea
              rows={4}
              required
              value={leadWallets}
              onChange={(e) => setLeadWallets(e.target.value)}
              placeholder="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"
              className="input"
              style={{ width: "100%", fontFamily: "var(--mono)", fontSize: "0.85rem" }}
            />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr", gap: "1.5rem", marginBottom: "1.5rem" }}>
            <div>
              <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                Copy Ratio (%)
              </label>
              <input
                type="number"
                min="1"
                max="500"
                value={copyRatioPct}
                onChange={(e) => setCopyRatioPct(Number(e.target.value))}
                className="input"
                style={{ width: "100%" }}
              />
              <span style={{ fontSize: "0.75rem", color: "var(--muted)" }}>50% = trade half leader size</span>
            </div>

            <div>
              <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                Max Trade Cap (SOL)
              </label>
              <input
                type="number"
                step="0.1"
                min="0.1"
                value={maxTradeSol}
                onChange={(e) => setMaxTradeSol(Number(e.target.value))}
                className="input"
                style={{ width: "100%" }}
              />
            </div>

            <div>
              <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
                Min Leader Balance (SOL)
              </label>
              <input
                type="number"
                min="1"
                value={minLeadBalanceSol}
                onChange={(e) => setMinLeadBalanceSol(Number(e.target.value))}
                className="input"
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div style={{ marginBottom: "1.5rem" }}>
            <label style={{ display: "flex", alignItems: "center", gap: "0.5rem", cursor: "pointer" }}>
              <input
                type="checkbox"
                checked={followSells}
                onChange={(e) => setFollowSells(e.target.checked)}
              />
              <span style={{ fontSize: "0.9rem" }}>Automatically mirror token sells when leader dumps</span>
            </label>
          </div>

          <button type="submit" disabled={loading} className="btn btn-primary">
            {loading ? "Saving Configuration..." : "Save Copy Trading Configuration"}
          </button>
        </form>
      </div>
    </AppShell>
  );
}
