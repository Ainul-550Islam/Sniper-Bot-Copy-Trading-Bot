"use client";

/**
 * Tenant-only PnL summary card (PROMPT 5 §K, file 92).
 *
 * Realized PnL for TODAY (UTC) from `GET /api/tenant/reports/pnl` —
 * computed server-side over the caller's own ledger. There is no
 * client-side math and no synthetic number: while the request is in
 * flight the card says so, and a refusal shows the honest reason.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { sessionStore } from "@/lib/auth";
import {
  classifyTradingError,
  customerTrading,
  isRetryable,
  PnlResponse,
  TradingSurfaceState,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

export default function PnlCard() {
  const [pnl, setPnl] = useState<PnlResponse | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });
  const [sessionVersion, setSessionVersion] = useState(0);
  const selectedOrganizationId = sessionStore.getSnapshot().selectedOrganizationId;
  const requestVersion = useRef(0);

  useEffect(() => sessionStore.subscribe(() => {
    requestVersion.current += 1;
    setSessionVersion((version) => version + 1);
    setPnl(null);
    setState({ kind: "loading" });
  }), []);

  const load = useCallback(async () => {
    if (selectedOrganizationId !== sessionStore.getSnapshot().selectedOrganizationId) return;
    const requestId = ++requestVersion.current;
    setPnl(null);
    if (!selectedOrganizationId) {
      setState({ kind: "empty" });
      return;
    }
    setState({ kind: "loading" });
    try {
      const response = await customerTrading.pnlToday();
      if (requestId !== requestVersion.current) return;
      if (response.organization_id !== selectedOrganizationId) {
        throw new Error("PnL response does not match the selected organization");
      }
      setPnl(response);
      setState({ kind: "ready" });
    } catch (e) {
      if (requestId === requestVersion.current) {
        setPnl(null);
        setState(classifyTradingError(e));
      }
    }
  }, [selectedOrganizationId]);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void load();
    });
    return () => {
      active = false;
      requestVersion.current += 1;
    };
  }, [load, sessionVersion]);

  const value =
    selectedOrganizationId && state.kind === "ready" && pnl?.organization_id === selectedOrganizationId ? (
      <>
        <strong className={pnl.pnl >= 0 ? "positive" : "negative"}>{pnl.pnl.toFixed(4)}</strong>{" "}
        <span className="muted">USD, realized today (UTC)</span>
      </>
    ) : null;

  return (
    <section className="card" aria-label="Realized PnL">
      <h2>Realized PnL — today</h2>
      {!selectedOrganizationId && <p>Select an organization to view realized PnL.</p>}
      {selectedOrganizationId && state.kind === "loading" && <p>Computing from your ledger…</p>}
      {selectedOrganizationId && state.kind === "ready" && value}
      {selectedOrganizationId && state.kind !== "loading" && state.kind !== "ready" && state.kind !== "empty" && (
        <p role="alert" className="error">
          {tradingStateMessage(state)}
          {isRetryable(state) && (
            <>
              <br />
              <button onClick={() => void load()} className="link">
                Retry
              </button>
            </>
          )}
        </p>
      )}
    </section>
  );
}
