"use client";

import { useEffect, useState, useCallback } from "react";
import Link from "next/link";
import {
  getHealth,
  getDbStatus,
  getModules,
  getExecutions,
  getExecutionById,
  getTrades,
  getWallets,
  getAccountingEvents,
  postAccountingEvent,
  getOrderById,
  getEventsWsPath,
  type HealthResponse,
  type DbStatus,
  type ModuleStatus,
  type ExecutionsResponse,
  type WalletsResponse,
  type AccountingEventsResponse,
} from "@/lib/api/operator-api";

export default function OperatorOverviewPage() {
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [db, setDb] = useState<DbStatus | null>(null);
  const [modules, setModules] = useState<ModuleStatus[]>([]);
  const [executions, setExecutions] = useState<ExecutionsResponse | null>(null);
  const [trades, setTrades] = useState<unknown[]>([]);
  const [wallets, setWallets] = useState<WalletsResponse | null>(null);
  const [accountingEvents, setAccountingEvents] = useState<AccountingEventsResponse | null>(null);

  // One-off lookup forms
  const [searchExecutionId, setSearchExecutionId] = useState("");
  const [executionResult, setExecutionResult] = useState<Record<string, unknown> | null>(null);
  const [searchOrderId, setSearchOrderId] = useState("");
  const [orderResult, setOrderResult] = useState<Record<string, unknown> | null>(null);

  // Accounting event post form
  const [eventKind, setEventKind] = useState("deposit");
  const [eventWallet, setEventWallet] = useState("treasury");
  const [eventQuoteAsset, setEventQuoteAsset] = useState("USDC");
  const [eventRef, setEventRef] = useState("");
  const [eventNotice, setEventNotice] = useState<string | null>(null);

  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    try {
      const [h, d, m, ex, tr, w, ae] = await Promise.all([
        getHealth().catch(() => ({ ok: false })),
        getDbStatus().catch(() => ({ database: "disabled" })),
        getModules().catch(() => []),
        getExecutions({ limit: 10 }).catch(() => ({ count: 0, states: {}, executions: [] })),
        getTrades(10).catch(() => []),
        getWallets().catch(() => ({ available: false, wallets: [] })),
        getAccountingEvents(10).catch(() => ({ count: 0, total: 0, by_kind: {}, pending: 0, events: [] })),
      ]);
      setHealth(h);
      setDb(d);
      setModules(m);
      setExecutions(ex);
      setTrades(tr);
      setWallets(w);
      setAccountingEvents(ae);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadData());
  }, [loadData]);

  const handleLookupExecution = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!searchExecutionId.trim()) return;
    try {
      const res = await getExecutionById(searchExecutionId.trim());
      setExecutionResult(res);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleLookupOrder = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!searchOrderId.trim()) return;
    try {
      const res = await getOrderById(searchOrderId.trim());
      setOrderResult(res);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const handlePostEvent = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!eventRef.trim()) return;
    try {
      const res = await postAccountingEvent({
        kind: eventKind,
        wallet: eventWallet.trim(),
        quote_asset: eventQuoteAsset.trim(),
        reference_id: eventRef.trim(),
      });
      setEventNotice(`Event recorded: outcome=${res.outcome}, event_id=${res.event_id}`);
      setEventRef("");
      await loadData();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const healthyModules = modules.filter((m) => m.healthy).length;
  const runningModules = modules.filter((m) => m.running).length;

  return (
    <article className="stack">
      <div className="row-between">
        <div>
          <h1>Operator System Overview</h1>
          <p className="muted">
            Global deployment health, database connection pool, module supervisor, and operational telemetry.
          </p>
        </div>
        <div className="row" style={{ alignItems: "center", gap: "0.5rem" }}>
          <span className="badge" title="WebSocket Event Stream">
            Feed: <code>{getEventsWsPath()}</code>
          </span>
          <button type="button" onClick={() => void loadData()} disabled={loading}>
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>

      {error && (
        <div className="card" style={{ borderLeft: "4px solid var(--bad)" }}>
          <p style={{ color: "var(--bad)", margin: 0 }}>{error}</p>
        </div>
      )}

      {/* Primary KPI Grid */}
      <div className="grid-4">
        <div className="card">
          <h4>API Liveness (/api/health)</h4>
          <p style={{ fontSize: "1.4rem", fontWeight: 700, margin: "0.5rem 0" }}>
            {health?.ok ? (
              <span style={{ color: "var(--good)" }}>HEALTHY</span>
            ) : (
              <span style={{ color: "var(--bad)" }}>UNHEALTHY</span>
            )}
          </p>
          <span className="tag tag--active">Axum Control Plane</span>
        </div>

        <div className="card">
          <h4>Database State (/api/db)</h4>
          <p style={{ fontSize: "1.4rem", fontWeight: 700, margin: "0.5rem 0" }}>
            {db?.database === "connected" ? (
              <span style={{ color: "var(--good)" }}>CONNECTED</span>
            ) : (
              <span className="muted">DISABLED</span>
            )}
          </p>
          <p className="small muted" style={{ margin: 0 }}>
            Pool active: {db?.pool?.active ?? 0} / {db?.pool?.max ?? 0}
          </p>
        </div>

        <div className="card">
          <h4>Supervisor Modules (/api/modules)</h4>
          <p style={{ fontSize: "1.4rem", fontWeight: 700, margin: "0.5rem 0" }}>
            {healthyModules} / {modules.length} Healthy
          </p>
          <p className="small muted" style={{ margin: 0 }}>
            {runningModules} running runtimes
          </p>
        </div>

        <div className="card">
          <h4>Audit Trail Durability</h4>
          <p style={{ fontSize: "1.4rem", fontWeight: 700, margin: "0.5rem 0" }}>
            {db?.audit_durable ? "Chained (DB)" : "Memory Only"}
          </p>
          <span className="tag">SHA-256 Ledger</span>
        </div>
      </div>

      {/* Operator Workspaces Grid */}
      <div className="card">
        <h2>Operator Control Workspaces</h2>
        <div className="grid-3" style={{ marginTop: "1rem" }}>
          <Link href="/operator/modules" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>⚙️ Module Controls</h3>
              <p className="muted small">Enable, disable, and monitor individual bot trading runtimes.</p>
            </div>
          </Link>

          <Link href="/operator/mode" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>🎯 Execution Mode</h3>
              <p className="muted small">Configure paper simulation, on-chain sim, or live real-money execution.</p>
            </div>
          </Link>

          <Link href="/operator/kill-switch" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>🛑 Emergency Stop</h3>
              <p className="muted small">Global system kill switch and granular venue/strategy risk breakers.</p>
            </div>
          </Link>

          <Link href="/operator/keys" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>🔑 Operator API Keys</h3>
              <p className="muted small">Provision and revoke scoped owner, operator, and readonly keys.</p>
            </div>
          </Link>

          <Link href="/operator/audit" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>📜 Audit Trail &amp; Verify</h3>
              <p className="muted small">Verify cryptographic hash integrity across mutating decisions.</p>
            </div>
          </Link>

          <Link href="/operator/journal" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>📝 Intent Journal</h3>
              <p className="muted small">Inspect JSONL journal sizes and execute rotation archive passes.</p>
            </div>
          </Link>

          <Link href="/operator/reconciliation" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>⚖️ Reconciliation Findings</h3>
              <p className="muted small">Inspect position, cash, and venue discrepancy findings.</p>
            </div>
          </Link>

          <Link href="/operator/recovery" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>🔄 Recovery Queue</h3>
              <p className="muted small">Inspect failed intents and automated recovery dead-letter items.</p>
            </div>
          </Link>

          <Link href="/operator/config" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>🔒 Runtime Config</h3>
              <p className="muted small">Inspect secrets-redacted runtime deployment configuration.</p>
            </div>
          </Link>
        </div>
      </div>

      {/* Execution Lifecycle & System Wallets */}
      <div className="grid-2">
        <div className="card stack">
          <h3>Execution Ledger (/api/executions)</h3>
          <p className="muted small">
            Recent transaction attempts in memory and durable ledger ({executions?.count ?? 0} listed).
          </p>
          <form onSubmit={(e) => void handleLookupExecution(e)} className="row" style={{ gap: "0.5rem" }}>
            <input
              type="text"
              placeholder="Lookup /api/executions/:id"
              value={searchExecutionId}
              onChange={(e) => setSearchExecutionId(e.target.value)}
              style={{ flex: 1 }}
            />
            <button type="submit" className="primary" disabled={!searchExecutionId.trim()}>
              Find
            </button>
          </form>
          {executionResult && (
            <pre style={{ background: "var(--panel-2)", padding: "0.5rem", fontSize: "0.75rem", maxHeight: "200px", overflow: "auto" }}>
              {JSON.stringify(executionResult, null, 2)}
            </pre>
          )}
        </div>

        <div className="card stack">
          <h3>Order Inspector (/api/orders/:id)</h3>
          <p className="muted small">Query OMS order manager or durable log by order identifier.</p>
          <form onSubmit={(e) => void handleLookupOrder(e)} className="row" style={{ gap: "0.5rem" }}>
            <input
              type="text"
              placeholder="Order ID (e.g. ord-123)"
              value={searchOrderId}
              onChange={(e) => setSearchOrderId(e.target.value)}
              style={{ flex: 1 }}
            />
            <button type="submit" className="primary" disabled={!searchOrderId.trim()}>
              Lookup
            </button>
          </form>
          {orderResult && (
            <pre style={{ background: "var(--panel-2)", padding: "0.5rem", fontSize: "0.75rem", maxHeight: "200px", overflow: "auto" }}>
              {JSON.stringify(orderResult, null, 2)}
            </pre>
          )}
        </div>
      </div>

      {/* Trades & Wallets */}
      <div className="grid-2">
        <div className="card stack">
          <h3>Recent Trades (/api/trades)</h3>
          <p className="muted small">Cross-module trade executions ({trades.length} recent).</p>
          <div style={{ maxHeight: "200px", overflow: "auto" }}>
            {trades.length === 0 ? (
              <p className="muted small">No trades recorded yet.</p>
            ) : (
              <pre style={{ background: "var(--panel-2)", padding: "0.5rem", fontSize: "0.75rem" }}>
                {JSON.stringify(trades, null, 2)}
              </pre>
            )}
          </div>
        </div>

        <div className="card stack">
          <h3>System Wallets (/api/wallets)</h3>
          <p className="muted small">Configured custody &amp; hot wallets in deployment database.</p>
          <p className="small">
            Available: <strong>{String(wallets?.available ?? false)}</strong> · Count: {wallets?.wallets?.length ?? 0}
          </p>
          {wallets?.wallets && wallets.wallets.length > 0 && (
            <pre style={{ background: "var(--panel-2)", padding: "0.5rem", fontSize: "0.75rem", maxHeight: "150px", overflow: "auto" }}>
              {JSON.stringify(wallets.wallets, null, 2)}
            </pre>
          )}
        </div>
      </div>

      {/* Financial Accounting Event Booking */}
      <div className="card stack">
        <h3>Book Accounting Event (/api/accounting/events)</h3>
        <p className="muted small">
          Record operator deposits, withdrawals, transfers, and corrections ({accountingEvents?.total ?? 0} total events).
        </p>

        {eventNotice && (
          <div className="card" style={{ borderLeft: "4px solid var(--good)" }}>
            <p style={{ margin: 0 }}>{eventNotice}</p>
          </div>
        )}

        <form onSubmit={(e) => void handlePostEvent(e)} className="grid-4" style={{ gap: "0.5rem" }}>
          <div>
            <label className="small muted">Kind</label>
            <select value={eventKind} onChange={(e) => setEventKind(e.target.value)}>
              <option value="deposit">deposit</option>
              <option value="withdrawal">withdrawal</option>
              <option value="transfer">transfer</option>
              <option value="fee">fee</option>
              <option value="correction">correction</option>
            </select>
          </div>
          <div>
            <label className="small muted">Wallet</label>
            <input value={eventWallet} onChange={(e) => setEventWallet(e.target.value)} required />
          </div>
          <div>
            <label className="small muted">Quote Asset</label>
            <input value={eventQuoteAsset} onChange={(e) => setEventQuoteAsset(e.target.value)} required />
          </div>
          <div>
            <label className="small muted">Reference ID</label>
            <input value={eventRef} onChange={(e) => setEventRef(e.target.value)} placeholder="bank-ref-001" required />
          </div>
          <div style={{ gridColumn: "span 4" }}>
            <button type="submit" className="primary" disabled={!eventRef.trim()}>
              Book Financial Event
            </button>
          </div>
        </form>
      </div>
    </article>
  );
}
