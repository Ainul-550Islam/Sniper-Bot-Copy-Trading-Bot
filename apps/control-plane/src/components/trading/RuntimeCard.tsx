"use client";

/**
 * Tenant runtime / fence / health status card (PROMPT 5 §K, file 89).
 *
 * Shows the live per-module runtimes registered for the CALLER's
 * organization (`GET /api/tenant/bots` — the same registry the runtime
 * launches into). Empty list is the honest "no runtimes registered"
 * state, never a synthesized one. Fence/generation fields come straight
 * from the runtime handle; nothing here is derived client-side.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { sessionStore } from "@/lib/auth";
import {
  BotsResponse,
  classifyTradingError,
  customerTrading,
  TradingSurfaceState,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

export default function RuntimeCard() {
  const [bots, setBots] = useState<BotsResponse | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });
  const [sessionVersion, setSessionVersion] = useState(0);
  const selectedOrganizationId = sessionStore.getSnapshot().selectedOrganizationId;
  const requestVersion = useRef(0);

  useEffect(() => sessionStore.subscribe(() => {
    requestVersion.current += 1;
    setSessionVersion((version) => version + 1);
    setBots(null);
    setState({ kind: "loading" });
  }), []);

  const load = useCallback(async () => {
    if (selectedOrganizationId !== sessionStore.getSnapshot().selectedOrganizationId) return;
    const requestId = ++requestVersion.current;
    setBots(null);
    if (!selectedOrganizationId) {
      setState({ kind: "empty" });
      return;
    }
    setState({ kind: "loading" });
    try {
      const response = await customerTrading.bots();
      if (requestId !== requestVersion.current) return;
      if (response.organization_id !== selectedOrganizationId) {
        throw new Error("Runtime response does not match the selected organization");
      }
      setBots(response);
      setState({ kind: response.items.length === 0 ? "empty" : "ready" });
    } catch (e) {
      if (requestId === requestVersion.current) {
        setBots(null);
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

  return (
    <section className="card" aria-label="Runtime status">
      <h2>Runtimes &amp; fencing</h2>
      {!selectedOrganizationId && <p>Select an organization to view runtimes.</p>}
      {selectedOrganizationId && state.kind === "loading" && <p>Loading runtimes…</p>}
      {selectedOrganizationId && state.kind !== "loading" && state.kind !== "ready" && state.kind !== "empty" && (
        <p role="alert" className="error">
          {tradingStateMessage(state)}
          <br />
          <button onClick={() => void load()} className="link">
            Retry
          </button>
        </p>
      )}
      {selectedOrganizationId && state.kind === "empty" && (
        <p>
          No bot runtimes are registered for your organization yet. This is the live registry —
          nothing is simulated here.
        </p>
      )}
      {selectedOrganizationId && state.kind === "ready" && bots?.organization_id === selectedOrganizationId && bots && (
        <table>
          <thead>
            <tr>
              <th>Module</th>
              <th>Runtime</th>
              <th>Generation (fence)</th>
              <th>Phase</th>
              <th>Since</th>
            </tr>
          </thead>
          <tbody>
            {bots.items.map((b) => (
              <tr key={`${b.module}:${b.runtime_id}`}>
                <td>{b.module}</td>
                <td>
                  <code>{b.runtime_id.slice(0, 8)}…</code>
                </td>
                <td>
                  <code>{b.generation.slice(0, 8)}…</code>
                </td>
                <td>{b.phase}</td>
                <td>{new Date(b.since).toLocaleString()}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {selectedOrganizationId && state.kind === "ready" && bots?.organization_id === selectedOrganizationId && (
        <p className="muted">
          Lifecycle transitions are runtime-owned and fenced by generation: this surface reports
          state, it does not reach around the fence.
        </p>
      )}
    </section>
  );
}
