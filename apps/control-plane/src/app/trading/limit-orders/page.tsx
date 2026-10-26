"use client";

import { useCallback, useEffect, useState } from "react";
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
  const organization =
    state.organizations.find((item) => item.organization_id === state.selectedOrganizationId) ??
    state.organizations[0];
  const role = organization?.role.toLowerCase() ?? "";
  if (role.includes("owner")) return "owner";
  if (role.includes("admin")) return "admin";
  if (role.includes("operator") || role.includes("trader")) return "operator";
  if (role.includes("analyst") || role.includes("auditor")) return "analyst";
  return "viewer";
}

export default function LimitOrdersPage() {
  const [, setSessionTick] = useState(0);
  const [orders, setOrders] = useState<LimitOrder[] | null>(null);
  const [nextBefore, setNextBefore] = useState<string | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });
  const [actionError, setActionError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [busyOrderId, setBusyOrderId] = useState<string | null>(null);
  const [cancelTarget, setCancelTarget] = useState<LimitOrder | null>(null);

  useEffect(() => sessionStore.subscribe(() => setSessionTick((value) => value + 1)), []);
  const canManage = getCapabilities({ role: currentRole() }).canCreateStrategies;

  const load = useCallback(async (before?: string) => {
    setState({ kind: "loading" });
    try {
      const response = await limitOrdersApi.list(before ? { before } : {});
      setOrders(response.orders);
      setNextBefore(response.next_before);
      setState({ kind: response.orders.length === 0 ? "empty" : "ready" });
    } catch (error) {
      setState(classifyTradingError(error));
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  async function create(input: CreateLimitOrderInput) {
    setCreating(true);
    setActionError(null);
    try {
      await limitOrdersApi.create(input);
      await load();
    } catch (error) {
      setActionError(tradingStateMessage(classifyTradingError(error)));
    } finally {
      setCreating(false);
    }
  }

  async function cancel(order: LimitOrder) {
    setBusyOrderId(order.id);
    setActionError(null);
    try {
      await limitOrdersApi.cancel(order.id);
      setCancelTarget(null);
      // Server truth only: the cancelled status appears when the refetch
      // returns it, never as an optimistic local rewrite.
      await load();
    } catch (error) {
      setActionError(tradingStateMessage(classifyTradingError(error)));
    } finally {
      setBusyOrderId(null);
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
