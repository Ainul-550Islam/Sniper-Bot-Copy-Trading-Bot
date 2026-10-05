"use client";

/**
 * Copy Trading Commercial Product Page (PROMPT 5 §K, file 86 & Commercial Readiness).
 *
 * Real-time leader tracking, swap decoding, mirror status, and risk gates.
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
  CopyConfig,
  TradingSurfaceState,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

interface LeaderRow {
  address: string;
  followed_since?: string;
  note?: string;
}

export default function CopyPage() {
  const [status, setStatus] = useState<ModuleStatusResponse | null>(null);
  const [config, setConfig] = useState<CopyConfig | null>(null);
  const [leaders, setLeaders] = useState<LeaderRow[]>([]);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });

  const loadData = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const [st, cfg, ldr] = await Promise.all([
        customerTrading.copyStatus(),
        customerTrading.copyConfig(),
        customerTrading.copyLeaders().catch(() => ({ organization_id: "", items: [] })),
      ]);
      setStatus(st);
      setConfig(cfg);
      setLeaders(ldr.items ?? []);
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
            <h1>Real-Time Solana Copy Trading</h1>
            <p className="muted">
              Mirror high-performing leader wallets with automated trade scaling, anti-sandwich protection, and stop-loss rules.
            </p>
          </div>
          <div className="row">
            <Link href="/trading/copy/config">
              <button className="primary">Copy Parameters ⚙</button>
            </Link>
            <Link href="/trading/positions">
              <button>Open Positions</button>
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
            <span className="stat-card__title">Tracked Wallets</span>
            <span className="stat-card__value">{leaders.length}</span>
            <span className="muted small">Authoritative tenant-scoped watch list</span>
          </div>

          <div className="stat-card">
            <span className="stat-card__title">Allocation Sizing</span>
            <span className="stat-card__value">
              {config ? `${config.allocation_per_trade_sol} SOL` : "—"}
            </span>
            <span className="muted small">Max Portfolio Exposure: ${config?.max_exposure_usd ?? 0}</span>
          </div>

          <div className="stat-card">
            <span className="stat-card__title">Execution Mode</span>
            <span className="stat-card__value">
              {config?.dry_run ? "PAPER / DRY-RUN" : "LIVE MAINNET"}
            </span>
            <span className={`tag ${config?.dry_run ? "tag--paper" : "tag--active"}`} style={{ width: "fit-content", marginTop: "0.3rem" }}>
              {config?.dry_run ? "Simulation" : "Live Mirroring"}
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
                    <strong>Entitlement:</strong> <code>{status.entitlement.feature ?? "module.copy"}</code> ({status.entitlement.granted ? "Granted" : "Denied"})
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
                    module="copy"
                    action="enable"
                    label="Activate Copy Engine"
                    available={status.controls.available}
                    onDone={() => void loadData()}
                  />
                  <ModuleActionButton
                    module="copy"
                    action="disable"
                    label="Pause Copy Engine"
                    available={status.controls.available}
                    onDone={() => void loadData()}
                  />
                </div>
              )}
            </div>
          )}
        </section>

        {/* Tracked Leaders Table */}
        <section className="card">
          <div className="row-between">
            <h2>Tracked Leader Wallets</h2>
            <Link href="/trading/copy/config">
              <button className="small">+ Add / Manage Wallets</button>
            </Link>
          </div>
          {leaders.length === 0 ? (
            <p className="muted" style={{ marginTop: "1rem" }}>
              No leader wallets configured yet. Add target Solana wallet public keys to begin automated copy mirroring.
            </p>
          ) : (
            <table style={{ marginTop: "1rem" }}>
              <thead>
                <tr>
                  <th>Solana Address</th>
                  <th>Label / Note</th>
                  <th>Followed Since</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {leaders.map((l) => (
                  <tr key={l.address}>
                    <td>
                      <code>{l.address}</code>
                    </td>
                    <td>{l.note || "Primary Target"}</td>
                    <td>{l.followed_since ? new Date(l.followed_since).toLocaleDateString() : "Active"}</td>
                    <td>
                      <Link href={`https://solscan.io/account/${l.address}`} target="_blank" rel="noreferrer">
                        <button className="link">Solscan ↗</button>
                      </Link>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>
      </div>
    </AppShell>
  );
}
