"use client";

/**
 * Sniper Trading Bot Commercial Product Page (PROMPT 5 §K, file 85 & Commercial Readiness).
 *
 * Exposes live Raydium / Pump.fun launch detection, latency indicators,
 * dry-run / paper mode controls, risk gates, and fast strategy tuning.
 */

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { AppShell } from "@/components/AppShell";
import ModuleActionButton from "@/components/trading/ModuleActionButton";
import {
  classifyTradingError,
  customerTrading,
  isRetryable,
  ModuleStatusResponse,
  SniperConfig,
  TradingSurfaceState,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

export default function SniperPage() {
  const [status, setStatus] = useState<ModuleStatusResponse | null>(null);
  const [config, setConfig] = useState<SniperConfig | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });

  const loadData = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const [st, cfg] = await Promise.all([
        customerTrading.sniperStatus(),
        customerTrading.sniperConfig(),
      ]);
      setStatus(st);
      setConfig(cfg);
      if (st.effective_state === "disabled") {
        setState({
          kind: "module_disabled",
          reason: st.tenant_override?.reason ?? "",
        });
      } else if (st.runtime === null) {
        setState({
          kind: "stale_runtime",
          reason: st.runtime_detail ?? "no runtime registered",
        });
      } else {
        setState({ kind: "ready" });
      }
    } catch (e) {
      setState(classifyTradingError(e));
    }
  }, []);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  return (
    <AppShell>
      <div className="stack">
        <div className="row-between">
          <div>
            <h1>Solana AMM &amp; Pump.fun Sniper Bot</h1>
            <p className="muted">
              Sub-second launch detection, Yellowstone Geyser gRPC feed, and MEV-protected execution.
            </p>
          </div>
          <div className="row">
            <Link href="/trading/sniper/config">
              <button className="primary">Strategy Parameters ⚙</button>
            </Link>
            <Link href="/backtests">
              <button>Backtest Launches</button>
            </Link>
          </div>
        </div>

        {/* State Alerts */}
        {state.kind !== "loading" && state.kind !== "ready" && (
          <div className="notice warn" role="alert">
            <p>{tradingStateMessage(state)}</p>
            {isRetryable(state) && (
              <button onClick={() => void loadData()} className="link" style={{ marginTop: "0.4rem" }}>
                Retry connection
              </button>
            )}
          </div>
        )}

        {/* Status Metrics */}
        <div className="grid-3">
          <div className="stat-card">
            <span className="stat-card__title">Execution Mode</span>
            <span className="stat-card__value">
              {config ? (config.dry_run ? "PAPER / DRY-RUN" : "LIVE MODE CONFIGURED") : "UNCONFIGURED"}
            </span>
            <span className={`tag ${config?.dry_run ? "tag--paper" : "tag--active"}`} style={{ width: "fit-content", marginTop: "0.3rem" }}>
              {config ? (config.dry_run ? "Paper mode configured" : "Live mode configured") : "No configuration recorded"}
            </span>
          </div>

          <div className="stat-card">
            <span className="stat-card__title">DEX Routing &amp; Engine</span>
            <span className="stat-card__value">
              {config ? config.dex_routing.toUpperCase() : "—"}
            </span>
            <span className="muted small">Priority: {config ? `${(config.priority_fee_lamports / 1e9).toFixed(4)} SOL` : "—"}</span>
          </div>

          <div className="stat-card">
            <span className="stat-card__title">Feed Latency (Geyser)</span>
            <span className="stat-card__value">Unavailable</span>
            <span className="muted small">No health-registry latency sample is exposed by this deployment</span>
          </div>
        </div>

        {/* Module Controls & Fencing */}
        <section className="card">
          <h2>Module Controls &amp; Fencing Authority</h2>
          {status && (
            <div className="stack" style={{ marginTop: "0.8rem" }}>
              <div className="grid-2">
                <div>
                  <p>
                    <strong>Entitlement:</strong> <code>{status.entitlement.feature ?? "module.sniper"}</code> ({status.entitlement.granted ? "Granted" : "Denied"})
                  </p>
                  <p>
                    <strong>Runtime Phase:</strong> <span className={`tag tag--${status.runtime?.phase}`}>{status.runtime?.phase ?? "idle"}</span>
                  </p>
                  <p>
                    <strong>Runtime ID:</strong> <code>{status.runtime?.runtime_id ?? "not scheduled"}</code>
                  </p>
                </div>
                <div>
                  <p>
                    <strong>Fence Generation:</strong> <code>{status.runtime?.generation ?? "—"}</code>
                  </p>
                  <p>
                    <strong>Effective State:</strong> <span className={`tag tag--${status.effective_state}`}>{status.effective_state}</span>
                  </p>
                  {status.tenant_override && (
                    <p className="muted small">
                      Override: {status.tenant_override.state} by {status.tenant_override.updated_by} ({status.tenant_override.reason || "no reason"})
                    </p>
                  )}
                </div>
              </div>

              {status.controls.available && (
                <div className="row" style={{ marginTop: "0.5rem" }}>
                  <ModuleActionButton
                    module="sniper"
                    action="enable"
                    label="Activate Sniper Engine"
                    available={status.controls.available}
                    onDone={() => void loadData()}
                  />
                  <ModuleActionButton
                    module="sniper"
                    action="disable"
                    label="Pause Sniper Engine"
                    available={status.controls.available}
                    onDone={() => void loadData()}
                  />
                </div>
              )}
            </div>
          )}
        </section>

        {/* Strategy Parameters Preview */}
        {config && (
          <section className="card">
            <div className="row-between">
              <h2>Active Strategy Rules</h2>
              <Link href="/trading/sniper/config">
                <button className="small">Edit Configuration ✎</button>
              </Link>
            </div>
            <div className="grid-4" style={{ marginTop: "1rem" }}>
              <div className="stat-card" style={{ background: "var(--panel-2)" }}>
                <span className="stat-card__title">Entry Sizing</span>
                <span className="stat-card__value" style={{ fontSize: "1.2rem" }}>{config.entry_amount_sol} SOL</span>
              </div>
              <div className="stat-card" style={{ background: "var(--panel-2)" }}>
                <span className="stat-card__title">Min Liquidity</span>
                <span className="stat-card__value" style={{ fontSize: "1.2rem" }}>{config.min_liquidity_sol} SOL</span>
              </div>
              <div className="stat-card" style={{ background: "var(--panel-2)" }}>
                <span className="stat-card__title">Max Slippage</span>
                <span className="stat-card__value" style={{ fontSize: "1.2rem" }}>{(config.max_slippage_bps / 100).toFixed(2)}%</span>
              </div>
              <div className="stat-card" style={{ background: "var(--panel-2)" }}>
                <span className="stat-card__title">Take Profit / Stop Loss</span>
                <span className="stat-card__value" style={{ fontSize: "1.2rem" }}>+{config.take_profit_pct}% / -{config.stop_loss_pct}%</span>
              </div>
            </div>
          </section>
        )}
      </div>
    </AppShell>
  );
}
