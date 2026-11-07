"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { AppShell } from "@/components/AppShell";
import { CancelConfirm } from "@/components/dca/CancelConfirm";
import { RunsPanel } from "@/components/dca/RunsPanel";
import { ScheduleForm } from "@/components/dca/ScheduleForm";
import { ScheduleTable } from "@/components/dca/ScheduleTable";
import { sessionStore } from "@/lib/auth";
import {
  classifyTradingError,
  isRetryable,
  tradingStateMessage,
  type TradingSurfaceState,
} from "@/lib/customer-trading-api";
import {
  dcaApi,
  type CreateDcaScheduleInput,
  type DcaRun,
  type DcaSchedule,
} from "@/lib/api/dca-api";
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

export default function DcaPage() {
  const [sessionTick, setSessionTick] = useState(0);
  const tenantVersion = useRef(0);
  const loadRequestVersion = useRef(0);
  const runsRequestVersion = useRef(0);
  const [schedules, setSchedules] = useState<DcaSchedule[] | null>(null);
  const [state, setState] = useState<TradingSurfaceState>({ kind: "loading" });
  const [actionError, setActionError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [busyScheduleId, setBusyScheduleId] = useState<string | null>(null);
  const [cancelTarget, setCancelTarget] = useState<DcaSchedule | null>(null);
  const [runsSchedule, setRunsSchedule] = useState<DcaSchedule | null>(null);
  const [runs, setRuns] = useState<DcaRun[] | null>(null);
  const [runsLoading, setRunsLoading] = useState(false);
  const [runsError, setRunsError] = useState<string | null>(null);

  useEffect(() => sessionStore.subscribe(() => {
    tenantVersion.current += 1;
    loadRequestVersion.current += 1;
    runsRequestVersion.current += 1;
    setSessionTick((value) => value + 1);
    setSchedules(null);
    setState({ kind: "loading" });
    setActionError(null);
    setCreating(false);
    setBusyScheduleId(null);
    setCancelTarget(null);
    setRunsSchedule(null);
    setRuns(null);
    setRunsLoading(false);
    setRunsError(null);
  }), []);
  const capabilities = getCapabilities({ role: currentRole() });
  const canManage = capabilities.canCreateStrategies;

  const load = useCallback(async () => {
    const requestTenantVersion = tenantVersion.current;
    const requestVersion = ++loadRequestVersion.current;
    setSchedules(null);
    setState({ kind: "loading" });
    try {
      const response = await dcaApi.list();
      if (requestTenantVersion !== tenantVersion.current || requestVersion !== loadRequestVersion.current) return;
      setSchedules(response.schedules);
      setState({ kind: response.schedules.length === 0 ? "empty" : "ready" });
    } catch (error) {
      if (requestTenantVersion === tenantVersion.current && requestVersion === loadRequestVersion.current) {
        setSchedules(null);
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

  async function create(input: CreateDcaScheduleInput) {
    const requestTenantVersion = tenantVersion.current;
    setCreating(true);
    setActionError(null);
    try {
      await dcaApi.create(input);
      if (requestTenantVersion === tenantVersion.current) await load();
    } catch (error) {
      if (requestTenantVersion === tenantVersion.current) {
        setActionError(tradingStateMessage(classifyTradingError(error)));
      }
    } finally {
      if (requestTenantVersion === tenantVersion.current) setCreating(false);
    }
  }

  async function transition(schedule: DcaSchedule, action: "pause" | "resume" | "cancel") {
    const requestTenantVersion = tenantVersion.current;
    setBusyScheduleId(schedule.id);
    setActionError(null);
    try {
      if (action === "pause") await dcaApi.pause(schedule.id);
      if (action === "resume") await dcaApi.resume(schedule.id);
      if (action === "cancel") await dcaApi.cancel(schedule.id);
      if (requestTenantVersion !== tenantVersion.current) return;
      if (action === "cancel") setCancelTarget(null);
      await load();
    } catch (error) {
      if (requestTenantVersion === tenantVersion.current) {
        setActionError(tradingStateMessage(classifyTradingError(error)));
      }
    } finally {
      if (requestTenantVersion === tenantVersion.current) setBusyScheduleId(null);
    }
  }

  async function showRuns(schedule: DcaSchedule) {
    const requestTenantVersion = tenantVersion.current;
    const requestVersion = ++runsRequestVersion.current;
    setRunsSchedule(schedule);
    setRuns(null);
    setRunsError(null);
    setRunsLoading(true);
    try {
      const response = await dcaApi.runs(schedule.id);
      if (requestTenantVersion === tenantVersion.current && requestVersion === runsRequestVersion.current) {
        setRuns(response.runs);
      }
    } catch (error) {
      if (requestTenantVersion === tenantVersion.current && requestVersion === runsRequestVersion.current) {
        setRunsError(tradingStateMessage(classifyTradingError(error)));
      }
    } finally {
      if (requestTenantVersion === tenantVersion.current && requestVersion === runsRequestVersion.current) {
        setRunsLoading(false);
      }
    }
  }

  return (
    <AppShell>
      <div className="stack">
        <div className="row-between">
          <div>
            <h1>DCA Schedules</h1>
            <p className="muted">
              Dollar-cost-averaging schedules recorded by the server. A schedule records intent
              for each interval; this page never treats a recorded run as a fill.
            </p>
          </div>
          <button type="button" onClick={() => void load()}>Refresh</button>
        </div>

        {actionError && <div className="notice danger" role="alert">{actionError}</div>}

        <ScheduleForm canManage={canManage} busy={creating} onCreate={create} />

        <section className="card" aria-label="DCA schedules">
          <h2>Schedules</h2>
          {state.kind === "loading" && <p className="muted">Loading DCA schedules…</p>}
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
          {state.kind === "empty" && <ScheduleTable schedules={[]} canManage={canManage} busyScheduleId={null} onPause={() => {}} onResume={() => {}} onRequestCancel={() => {}} onShowRuns={() => {}} />}
          {state.kind === "ready" && schedules && (
            <ScheduleTable
              schedules={schedules}
              canManage={canManage}
              busyScheduleId={busyScheduleId}
              onPause={(schedule) => void transition(schedule, "pause")}
              onResume={(schedule) => void transition(schedule, "resume")}
              onRequestCancel={setCancelTarget}
              onShowRuns={(schedule) => void showRuns(schedule)}
            />
          )}
        </section>

        {cancelTarget && (
          <CancelConfirm
            schedule={cancelTarget}
            busy={busyScheduleId === cancelTarget.id}
            onConfirm={() => void transition(cancelTarget, "cancel")}
            onDismiss={() => setCancelTarget(null)}
          />
        )}

        {runsSchedule && (
          <RunsPanel
            schedule={runsSchedule}
            runs={runs}
            loading={runsLoading}
            error={runsError}
            onClose={() => {
              runsRequestVersion.current += 1;
              setRunsSchedule(null);
              setRuns(null);
              setRunsLoading(false);
              setRunsError(null);
            }}
          />
        )}
      </div>
    </AppShell>
  );
}
