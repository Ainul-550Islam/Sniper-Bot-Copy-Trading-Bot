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

import { useCallback, useEffect, useRef, useState } from "react";
import ModuleActionButton from "@/components/trading/ModuleActionButton";
import { sessionStore } from "@/lib/auth";
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
  const [sessionVersion, setSessionVersion] = useState(0);
  const selectedOrganizationId = sessionStore.getSnapshot().selectedOrganizationId;
  const requestVersion = useRef(0);

  useEffect(() => sessionStore.subscribe(() => {
    requestVersion.current += 1;
    setSessionVersion((version) => version + 1);
    setStatus(null);
    setState({ kind: "loading" });
  }), []);

  const load = useCallback(async () => {
    if (selectedOrganizationId !== sessionStore.getSnapshot().selectedOrganizationId) return;
    const requestId = ++requestVersion.current;
    setStatus(null);
    if (!selectedOrganizationId) {
      setState({ kind: "empty" });
      return;
    }
    setState({ kind: "loading" });
    try {
      const response = await customerTrading[`${module}Status`]();
      if (requestId !== requestVersion.current) return;
      if (response.organization_id !== selectedOrganizationId) {
        throw new Error("Module status response does not match the selected organization");
      }
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
      if (requestId === requestVersion.current) {
        setStatus(null);
        setState(classifyTradingError(e));
      }
    }
  }, [module, selectedOrganizationId]);

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
    <section className="card" aria-label={`${title} status`}>
      <h3>{title}</h3>
      {!selectedOrganizationId && <p>Select an organization to view module status.</p>}
      {selectedOrganizationId && state.kind === "loading" && <p>Loading {title} status…</p>}
      {selectedOrganizationId && state.kind !== "loading" && state.kind !== "ready" && state.kind !== "empty" && (
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
      {selectedOrganizationId && state.kind === "ready" && status?.organization_id === selectedOrganizationId && status && (
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
      {selectedOrganizationId && status?.organization_id === selectedOrganizationId && status.controls.available && (
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
