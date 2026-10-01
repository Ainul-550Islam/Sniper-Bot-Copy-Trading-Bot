"use client";

/**
 * Copy trading configuration/status page (PROMPT 5 §K, file 86).
 *
 * Module controls plus the caller's followed leaders (`/api/tenant/copy/leaders`).
 */

import { useCallback, useEffect, useState } from "react";
import ModuleCards from "@/components/trading/ModuleCards";
import { tenantRequest, classifyTradingError, isRetryable, TradingSurfaceState, tradingStateMessage } from "@/lib/customer-trading-api";

interface LeaderRow {
  address: string;
  followed_since?: string;
  note?: string;
}

export default function CopyPage() {
  const [leaders, setLeaders] = useState<LeaderRow[] | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });

  const load = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const response = await tenantRequest<{ leaders?: LeaderRow[]; items?: LeaderRow[] } | LeaderRow[]>("/api/tenant/copy/leaders");
      const list = Array.isArray(response) ? response : response.leaders ?? response.items ?? [];
      setLeaders(list);
      setState({ kind: list.length === 0 ? "empty" : "ready" });
    } catch (e) {
      setState(classifyTradingError(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <main id="main" className="stack">
      <h1>Copy Trading</h1>
      <section className="card" aria-label="Followed leaders">
        <h2>Followed leaders</h2>
        {state.kind === "loading" && <p>Loading leaders…</p>}
        {state.kind !== "loading" && state.kind !== "ready" && state.kind !== "empty" && (
          <p role="alert" className="error">
            {tradingStateMessage(state)}
            {isRetryable(state) && (
              <>
                <br />
                <button onClick={() => void load()} className="link">Retry</button>
              </>
            )}
          </p>
        )}
        {state.kind === "empty" && <p>You are not following any leaders yet. Leaders you follow appear here — never another tenant&apos;s.</p>}
        {state.kind === "ready" && leaders && (
          <ul>
            {leaders.map((l) => (
              <li key={l.address}>
                <code>{l.address}</code>
                {l.followed_since ? ` — since ${new Date(l.followed_since).toLocaleDateString()}` : ""}
              </li>
            ))}
          </ul>
        )}
      </section>
      <ModuleCards />
    </main>
  );
}
