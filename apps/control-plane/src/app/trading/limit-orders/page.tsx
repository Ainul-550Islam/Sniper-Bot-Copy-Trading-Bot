"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { AppShell } from "@/components/AppShell";
import { CancelConfirm } from "@/components/limit-orders/CancelConfirm";
import { OrderForm } from "@/components/limit-orders/OrderForm";
import { OrderTable } from "@/components/limit-orders/OrderTable";
import { sessionStore } from "@/lib/auth";
import {
  classifyTradingError,
  isRetryable,
  tradingStateMessage,
  type TradingSurfaceState,
} from "@/lib/customer-trading-api";
import {
  limitOrdersApi,
  type CreateLimitOrderInput,
  type LimitOrder,
} from "@/lib/api/limit-orders-api";
import { getCapabilities, type MembershipRole } from "@/lib/permissions";

function currentRole(): MembershipRole {
  const state = sessionStore.getSnapshot();
  const organization = state.selectedOrganizationId
    ? state.organizations.find((item) => item.organization_id === state.selectedOrganizationId)
    : state.organizations.length === 1 ? state.organizations[0] : undefined;
  const role = organization?.role.toLowerCase() ?? "";
  if (role.includes("owner")) return "owner";
  if (role.includes("admin")) return "admin";
  if (role.includes("operator") || role.includes("trader")) return "operator";
  if (role.includes("analyst") || role.includes("auditor")) return "analyst";
  return "viewer";
}

export default function LimitOrdersPage() {
  const [sessionTick, setSessionTick] = useState(0);
  const tenantVersion = useRef(0);
  const loadRequestVersion = useRef(0);
  const [orders, setOrders] = useState<LimitOrder[] | null>(null);
  const [nextBefore, setNextBefore] = useState<string | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });
  const [actionError, setActionError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [busyOrderId, setBusyOrderId] = useState<string | null>(null);
  const [cancelTarget, setCancelTarget] = useState<LimitOrder | null>(null);

  useEffect(() => sessionStore.subscribe(() => {
    tenantVersion.current += 1;
    loadRequestVersion.current += 1;
    setSessionTick((value) => value + 1);
    setOrders(null);
    setNextBefore(null);
    setState({ kind: "loading" });
    setActionError(null);
    setCreating(false);
    setBusyOrderId(null);
    setCancelTarget(null);
  }), []);
  const canManage = getCapabilities({ role: currentRole() }).canCreateStrategies;

  const load = useCallback(async (before?: string) => {
    const requestTenantVersion = tenantVersion.current;
    const requestVersion = ++loadRequestVersion.current;
    setOrders(null);
    setNextBefore(null);
    setState({ kind: "loading" });
    try {
      const response = await limitOrdersApi.list(before ? { before } : {});
      if (requestTenantVersion !== tenantVersion.current || requestVersion !== loadRequestVersion.current) return;
      setOrders(response.orders);
      setNextBefore(response.next_before);
      setState({ kind: response.orders.length === 0 ? "empty" : "ready" });
    } catch (error) {
      if (requestTenantVersion === tenantVersion.current && requestVersion === loadRequestVersion.current) {
        setOrders(null);
        setNextBefore(null);
        setState(classifyTradingError(error));
      }
    }
  }, []);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void load();
    });
    return () => {
      active = false;
      loadRequestVersion.current += 1;
    };
  }, [load, sessionTick]);

  async function create(input: CreateLimitOrderInput) {
    const requestTenantVersion = tenantVersion.current;
    setCreating(true);
    setActionError(null);
    try {
      await limitOrdersApi.create(input);
      if (requestTenantVersion === tenantVersion.current) await load();
    } catch (error) {
      if (requestTenantVersion === tenantVersion.current) {
        setActionError(tradingStateMessage(classifyTradingError(error)));
      }
    } finally {
      if (requestTenantVersion === tenantVersion.current) setCreating(false);
    }
  }

  async function cancel(order: LimitOrder) {
    const requestTenantVersion = tenantVersion.current;
    setBusyOrderId(order.id);
    setActionError(null);
    try {
      await limitOrdersApi.cancel(order.id);
      if (requestTenantVersion !== tenantVersion.current) return;
      setCancelTarget(null);
      // Server truth only: the cancelled status appears when the refetch
      // returns it, never as an optimistic local rewrite.
      await load();
    } catch (error) {
      if (requestTenantVersion === tenantVersion.current) {
        setActionError(tradingStateMessage(classifyTradingError(error)));
      }
    } finally {
      if (requestTenantVersion === tenantVersion.current) setBusyOrderId(null);
    }
  }

  return (
    <AppShell>
      <div className="stack">
        <div className="row-between">
          <div>
            <h1>Limit Orders</h1>
            <p className="muted">
              Server-recorded limit-order intent. Execution is performed by the limit-order
              worker when a trigger is met; creating an order here does not move funds.
            </p>
          </div>
          <button type="button" onClick={() => void load()}>Refresh</button>
        </div>

        {actionError && <div className="notice danger" role="alert">{actionError}</div>}

        <OrderForm canManage={canManage} busy={creating} onCreate={create} />

        <section className="card" aria-label="Limit orders">
          <h2>Orders</h2>
          {state.kind === "loading" && <p className="muted">Loading limit orders…</p>}
          {state.kind !== "loading" && state.kind !== "ready" && state.kind !== "empty" && (
            <p role="alert" className="error">
              {tradingStateMessage(state)}
              {isRetryable(state) && (
                <>
                  <br />
                  <button type="button" className="link" onClick={() => void load()}>Retry</button>
                </>
              )}
            </p>
          )}
          {state.kind === "empty" && (
            <OrderTable orders={[]} canManage={canManage} busyOrderId={null} onRequestCancel={() => {}} />
          )}
          {state.kind === "ready" && orders && (
            <>
              <OrderTable
                orders={orders}
                canManage={canManage}
                busyOrderId={busyOrderId}
                onRequestCancel={setCancelTarget}
              />
              <nav className="row" style={{ marginTop: "1rem" }}>
                <button type="button" onClick={() => void load()}>Newest</button>
                <button
                  type="button"
                  disabled={nextBefore === null}
                  onClick={() => nextBefore && void load(nextBefore)}
                >
                  Older orders
                </button>
              </nav>
            </>
          )}
        </section>

        {cancelTarget && (
          <CancelConfirm
            order={cancelTarget}
            busy={busyOrderId === cancelTarget.id}
            onConfirm={() => void cancel(cancelTarget)}
            onDismiss={() => setCancelTarget(null)}
          />
        )}
      </div>
    </AppShell>
  );
}
