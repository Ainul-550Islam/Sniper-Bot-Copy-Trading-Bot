"use client";

/**
 * Tenant-only PnL summary card (PROMPT 5 §K, file 92).
 *
 * Realized PnL for TODAY (UTC) from `GET /api/tenant/reports/pnl` —
 * computed server-side over the caller's own ledger. There is no
 * client-side math and no synthetic number: while the request is in
 * flight the card says so, and a refusal shows the honest reason.
 */

import { useCallback, useEffect, useState } from "react";
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

  const load = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const response = await customerTrading.pnlToday();
      setPnl(response);
      setState({ kind: "ready" });
    } catch (e) {
      setState(classifyTradingError(e));
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  const value =
    state.kind === "ready" && pnl ? (
      <>
        <strong className={pnl.pnl >= 0 ? "positive" : "negative"}>{pnl.pnl.toFixed(4)}</strong>{" "}
        <span className="muted">USD, realized today (UTC)</span>
      </>
    ) : null;

  return (
    <section className="card" aria-label="Realized PnL">
      <h2>Realized PnL — today</h2>
      {state.kind === "loading" && <p>Computing from your ledger…</p>}
      {state.kind === "ready" && value}
      {state.kind !== "loading" && state.kind !== "ready" && (
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
