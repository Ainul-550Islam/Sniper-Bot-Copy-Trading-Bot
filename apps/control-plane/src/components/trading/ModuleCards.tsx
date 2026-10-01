"use client";

/**
 * Sniper / Copy / Polymarket runtime status cards (PROMPT 5 §K, file 93;
 * controls via the ModuleActionButton component since PROMPT 6 §I).
 *
 * One card per trading module, fed by that module's OWN status endpoint
 * (`/api/tenant/{module}/status`): entitlement (verified by the chain),
 * the tenant-level override, and the registered runtime phase. Every
 * non-ready state renders honestly — entitlement denial, tenant
 * disabled, stale runtime, plane unavailable — and the enable/disable
 * controls act ONLY on the caller's organization.
 */

import { useCallback, useEffect, useState } from "react";
import ModuleActionButton from "@/components/trading/ModuleActionButton";
import {
  classifyTradingError,
  customerTrading,
  isRetryable,
  ModuleStatusResponse,
  TradingSurfaceState,
  tradingStateMessage,
} from "@/lib/customer-trading-api";

type ModuleKey = "sniper" | "copy" | "polymarket";

interface ModuleCardProps {
  module: ModuleKey;
  title: string;
}

function ModuleCard({ module, title }: ModuleCardProps) {
  const [status, setStatus] = useState<ModuleStatusResponse | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });

  const load = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const response = await customerTrading[`${module}Status`]();
      setStatus(response);
      if (response.effective_state === "disabled") {
        setState({
          kind: "module_disabled",
          reason: response.tenant_override?.reason ?? "",
        });
      } else if (response.runtime === null) {
        setState({
          kind: "stale_runtime",
          reason: response.runtime_detail ?? "no runtime registered",
        });
      } else {
        setState({ kind: "ready" });
      }
    } catch (e) {
      setState(classifyTradingError(e));
    }
  }, [module]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <section className="card" aria-label={`${title} status`}>
      <h3>{title}</h3>
      {state.kind === "loading" && <p>Loading {title} status…</p>}
      {state.kind !== "loading" && state.kind !== "ready" && (
        <p role="alert" className={state.kind === "module_disabled" ? "warn" : "error"}>
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
      {state.kind === "ready" && status && (
        <ul>
          <li>
            Entitlement: <code>{status.entitlement.feature ?? "control plane"}</code> granted
          </li>
          <li>
            Runtime phase: <strong>{status.runtime?.phase}</strong> since{" "}
            {status.runtime ? new Date(status.runtime.since).toLocaleString() : "—"}
          </li>
          <li>
            Fence generation: <code>{status.runtime?.generation.slice(0, 8)}…</code>
          </li>
          {status.tenant_override && (
            <li>
              Tenant override: {status.tenant_override.state} — {status.tenant_override.reason || "no reason given"} (
              {status.tenant_override.updated_by})
            </li>
          )}
        </ul>
      )}
      {status?.controls.available && (
        <div className="row">
          <ModuleActionButton
            module={module}
            action="enable"
            label="Enable"
            available={status.controls.available}
            onDone={() => void load()}
          />
          <ModuleActionButton
            module={module}
            action="disable"
            label="Disable"
            available={status.controls.available}
            onDone={() => void load()}
          />
        </div>
      )}
      <p className="muted">
        Tenant-level controls for your organization only; runtime lifecycle transitions stay
        runtime-owned and fenced.
      </p>
    </section>
  );
}

export default function ModuleCards() {
  return (
    <div className="stack">
      <ModuleCard module="sniper" title="Sniper" />
      <ModuleCard module="copy" title="Copy Trading" />
      <ModuleCard module="polymarket" title="Polymarket" />
    </div>
  );
}
