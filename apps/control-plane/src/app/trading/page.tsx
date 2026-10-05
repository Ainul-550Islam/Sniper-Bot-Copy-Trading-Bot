"use client";

/**
 * Unified Commercial Trading Workspace (PROMPT 5 §K, file 81 & Commercial Readiness).
 *
 * Customer-only: every metric, bot status, and position on this page comes from
 * `/api/tenant/*` endpoints through `lib/customer-trading-api.ts`.
 * No synthetic demo data — each widget renders its own loading / empty / error state honestly.
 */

import { useEffect, useState, useCallback } from "react";
import Link from "next/link";
import { AppShell } from "@/components/AppShell";
import RuntimeCard from "@/components/trading/RuntimeCard";
import PnlCard from "@/components/trading/PnlCard";
import ModuleCards from "@/components/trading/ModuleCards";
import {
  customerTrading,
  classifyTradingError,
  TradingSurfaceState,
  tradingStateMessage,
  isRetryable,
  OrdersPage,
  PositionsPage,
} from "@/lib/customer-trading-api";

export default function TradingDashboardPage() {
  const [orders, setOrders] = useState<OrdersPage | null>(null);
  const [positions, setPositions] = useState<PositionsPage | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });

  const loadData = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const [ord, pos] = await Promise.all([
        customerTrading.orders(),
        customerTrading.positions(),
      ]);
      setOrders(ord);
      setPositions(pos);
      setState({ kind: "ready" });
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
            <h1>Trading Terminal &amp; Portfolio Workspace</h1>
            <p className="muted">
              Live overview of algorithmic strategies, active bot runtimes, open positions, and execution metrics.
            </p>
          </div>
          <div className="row">
            <Link href="/strategies">
              <button className="primary">+ New Strategy</button>
            </Link>
            <Link href="/backtests">
              <button>Run Backtest</button>
            </Link>
          </div>
        </div>

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

        {/* Top Summary Widgets */}
        <div className="grid-3">
          <PnlCard />
          <div className="stat-card">
            <span className="stat-card__title">Open Positions</span>
            <span className="stat-card__value">
              {positions ? positions.items.filter((p) => p.status === "open").length : "—"}
            </span>
            <span className="stat-card__trend up">
              <span>●</span> Live mark-to-market tracking
            </span>
          </div>
          <div className="stat-card">
            <span className="stat-card__title">Recent Orders (24h)</span>
            <span className="stat-card__value">
              {orders ? orders.items.length : "—"}
            </span>
            <span className="muted small">Authoritative tenant order book</span>
          </div>
        </div>

        {/* Bot Runtimes & Fencing */}
        <RuntimeCard />

        {/* Module Controllers */}
        <div className="card">
          <div className="row-between" style={{ marginBottom: "1rem" }}>
            <h2>Automated Trading Modules</h2>
            <span className="muted small">Fenced execution engines</span>
          </div>
          <ModuleCards />
        </div>

        {/* Quick Navigation to Detailed Books */}
        <div className="grid-3">
          <div className="card">
            <h3>Orders Book</h3>
            <p className="muted small">View, filter, and manage active or historical orders across all venues.</p>
            <Link href="/trading/orders" style={{ marginTop: "0.5rem", display: "inline-block" }}>
              <button>Open Orders →</button>
            </Link>
          </div>
          <div className="card">
            <h3>Positions &amp; Exposure</h3>
            <p className="muted small">Analyze open inventory, realized vs unrealized gains, and mark prices.</p>
            <Link href="/trading/positions" style={{ marginTop: "0.5rem", display: "inline-block" }}>
              <button>Open Positions →</button>
            </Link>
          </div>
          <div className="card">
            <h3>Execution Journal</h3>
            <p className="muted small">Inspect fill signatures, recorded latency traces, and recovery states.</p>
            <Link href="/trading/executions" style={{ marginTop: "0.5rem", display: "inline-block" }}>
              <button>Open Executions →</button>
            </Link>
          </div>
        </div>
      </div>
    </AppShell>
  );
}
