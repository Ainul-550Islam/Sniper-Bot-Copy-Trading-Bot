"use client";

/**
 * Tenant positions table (PROMPT 5 §K, file 91).
 *
 * Server-paginated caller-owned positions. PnL columns render exactly
 * what the server returned — a `null` stays &ldquo;—&rdquo;, never a
 * client-side zero or estimate.
 */

import Link from "next/link";
import { useCallback, useEffect, useRef, useState } from "react";
import { sessionStore } from "@/lib/auth";
import {
  classifyTradingError,
  customerTrading,
  isRetryable,
  PositionsPage,
  TradingSurfaceState,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

function fmt(value: number | null): string {
  return value === null ? "—" : value.toFixed(4);
}

export default function PositionTable() {
  const [page, setPage] = useState<PositionsPage | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });
  const [sessionVersion, setSessionVersion] = useState(0);
  const selectedOrganizationId = sessionStore.getSnapshot().selectedOrganizationId;
  const requestVersion = useRef(0);

  useEffect(() => sessionStore.subscribe(() => {
    requestVersion.current += 1;
    setSessionVersion((version) => version + 1);
    setPage(null);
    setCursor(null);
    setState({ kind: "loading" });
  }), []);

  const load = useCallback(async (nextCursor: string | null) => {
    if (selectedOrganizationId !== sessionStore.getSnapshot().selectedOrganizationId) return;
    const requestId = ++requestVersion.current;
    setPage(null);
    setState({ kind: "loading" });
    if (!selectedOrganizationId) {
      setCursor(null);
      setState({ kind: "empty" });
      return;
    }
    try {
      const response = await customerTrading.positions(nextCursor);
      if (requestId !== requestVersion.current) return;
      if (response.organization_id !== selectedOrganizationId) {
        throw new Error("Position response does not match the selected organization");
      }
      setPage(response);
      setCursor(nextCursor);
      setState({ kind: response.items.length === 0 ? "empty" : "ready" });
    } catch (e) {
      if (requestId === requestVersion.current) {
        setPage(null);
        setState(classifyTradingError(e));
      }
    }
  }, [selectedOrganizationId]);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void load(null);
    });
    return () => {
      active = false;
      requestVersion.current += 1;
    };
  }, [load, sessionVersion]);

  return (
    <section className="card" aria-label="Positions">
      <h2>Positions</h2>
      {!selectedOrganizationId && <p>Select an organization to view positions.</p>}
      {selectedOrganizationId && state.kind === "loading" && <p>Loading positions…</p>}
      {selectedOrganizationId && state.kind !== "loading" && state.kind !== "ready" && state.kind !== "empty" && (
        <p role="alert" className="error">
          {tradingStateMessage(state)}
          {isRetryable(state) && (
            <>
              <br />
              <button onClick={() => void load(cursor)} className="link">
                Retry
              </button>
            </>
          )}
        </p>
      )}
      {selectedOrganizationId && state.kind === "empty" && <p>No open or historical positions yet.</p>}
      {selectedOrganizationId && state.kind === "ready" && page?.organization_id === selectedOrganizationId && page && (
        <>
          <table>
            <thead>
              <tr>
                <th>Symbol</th>
                <th>Side</th>
                <th>Qty</th>
                <th>Entry</th>
                <th>Mark</th>
                <th>Realized PnL</th>
                <th>Unrealized PnL</th>
                <th>Status</th>
                <th>Opened</th>
                <th>Closed</th>
                <th>Details</th>
              </tr>
            </thead>
            <tbody>
              {page.items.map((p) => (
                <tr key={p.id}>
                  <td>{p.symbol}</td>
                  <td>{p.side}</td>
                  <td>{fmt(p.qty)}</td>
                  <td>{fmt(p.entry_price)}</td>
                  <td>{fmt(p.mark_price)}</td>
                  <td>{fmt(p.realized_pnl)}</td>
                  <td>{fmt(p.unrealized_pnl)}</td>
                  <td>{p.status}</td>
                  <td>{p.opened_at ? new Date(p.opened_at).toLocaleString() : "—"}</td>
                  <td>{p.closed_at ? new Date(p.closed_at).toLocaleString() : "—"}</td>
                  <td>
                    <Link href={`/trading/positions/${p.id}`}>Details</Link>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <nav className="row">
            <button disabled={cursor === null} onClick={() => void load(null)}>
              ⟲ First page
            </button>
            <button disabled={page.next_cursor === null} onClick={() => void load(page.next_cursor)}>
              Older positions →
            </button>
          </nav>
        </>
      )}
    </section>
  );
}
