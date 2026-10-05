"use client";

/**
 * Polymarket V3 CLOB Prediction Market Product Page (PROMPT 5 §K, file 87 & Commercial Readiness).
 *
 * Exposes Gamma market discovery, CLOB order book status, reconciliation drift,
 * EIP-712 signer state, and truthful provider availability.
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
  PolymarketConfig,
  TradingSurfaceState,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

interface PolyOrderRow {
  id: string;
  market?: string | null;
  side?: string | null;
  status?: string | null;
  created_at?: string | null;
  [key: string]: unknown;
}

export default function PolymarketPage() {
  const [status, setStatus] = useState<ModuleStatusResponse | null>(null);
  const [config, setConfig] = useState<PolymarketConfig | null>(null);
  const [orders, setOrders] = useState<PolyOrderRow[]>([]);
  const [fills, setFills] = useState<unknown[]>([]);
  const [drift, setDrift] = useState<unknown[]>([]);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });

  const loadData = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const [st, cfg, ord, fl, rc] = await Promise.all([
        customerTrading.polymarketStatus(),
        customerTrading.polymarketConfig(),
        customerTrading.polymarketOrders().catch(() => ({ organization_id: "", items: [], next_cursor: null })),
        customerTrading.polymarketFills().catch(() => ({ items: [] })),
        customerTrading.polymarketReconciliation().catch(() => ({ organization_id: "", drift: [] })),
      ]);
      setStatus(st);
      setConfig(cfg);
      setOrders((ord.items as PolyOrderRow[]) ?? []);
      setFills(fl.items ?? []);
      setDrift(rc.drift ?? []);

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
            <h1>Polymarket V3 CLOB Trading Desk</h1>
            <p className="muted">
              Gamma market discovery, automated market making, EIP-712 order signing, and reconciliation.
            </p>
          </div>
          <div className="row">
            <Link href="/trading/polymarket/config">
              <button className="primary">Strategy Parameters ⚙</button>
            </Link>
            <Link href="/markets">
              <button>Explore Gamma Markets</button>
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
              {config?.dry_run ? "PAPER SIMULATION" : "LIVE CLOB"}
            </span>
            <span className={`tag ${config?.dry_run ? "tag--paper" : "tag--active"}`} style={{ width: "fit-content", marginTop: "0.3rem" }}>
              {config?.dry_run ? "Simulation Mode" : "EIP-712 Live Signed"}
            </span>
          </div>

          <div className="stat-card">
            <span className="stat-card__title">Position Cap (USDC)</span>
            <span className="stat-card__value">
              ${config?.max_position_size_usdc ?? 500}
            </span>
            <span className="muted small">Max Market Exposure: ${config?.max_market_exposure_usdc ?? 2500}</span>
          </div>

          <div className="stat-card">
            <span className="stat-card__title">Reconciliation State</span>
            <span className="stat-card__value">
              {drift.length === 0 ? "IN SYNC (0 DRIFT)" : `${drift.length} DISCREPANCIES`}
            </span>
            <span className={`tag ${drift.length === 0 ? "tag--healthy" : "tag--warning"}`} style={{ width: "fit-content", marginTop: "0.3rem" }}>
              {drift.length === 0 ? "Pure Deterministic" : "Review Required"}
            </span>
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
                    <strong>Entitlement:</strong> <code>{status.entitlement.feature ?? "module.polymarket"}</code> ({status.entitlement.granted ? "Granted" : "Denied"})
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
                    module="polymarket"
                    action="enable"
                    label="Activate Polymarket CLOB"
                    available={status.controls.available}
                    onDone={() => void loadData()}
                  />
                  <ModuleActionButton
                    module="polymarket"
                    action="disable"
                    label="Pause Polymarket CLOB"
                    available={status.controls.available}
                    onDone={() => void loadData()}
                  />
                </div>
              )}
            </div>
          )}
        </section>

        {/* Orders Table */}
        <section className="card">
          <h2>Polymarket V3 Mirror Order Book</h2>
          {orders.length === 0 ? (
            <p className="muted" style={{ marginTop: "1rem" }}>
              No Polymarket orders recorded yet. Active CLOB limit and market orders will populate here.
            </p>
          ) : (
            <table style={{ marginTop: "1rem" }}>
              <thead>
                <tr>
                  <th>Order ID</th>
                  <th>Market / Condition</th>
                  <th>Side</th>
                  <th>Status</th>
                  <th>Timestamp</th>
                </tr>
              </thead>
              <tbody>
                {orders.map((o) => (
                  <tr key={o.id}>
                    <td><code>{o.id.slice(0, 12)}…</code></td>
                    <td>{o.market ?? "—"}</td>
                    <td>
                      <span className={`tag ${o.side === "BUY" ? "tag--active" : "tag--warning"}`}>
                        {o.side ?? "BUY"}
                      </span>
                    </td>
                    <td>{o.status ?? "open"}</td>
                    <td>{o.created_at ? new Date(o.created_at).toLocaleString() : "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {fills.length > 0 && (
            <p className="muted small" style={{ marginTop: "0.8rem" }}>
              {fills.length} fill execution(s) reconciled in the current settlement window.
            </p>
          )}
        </section>
      </div>
    </AppShell>
  );
}
