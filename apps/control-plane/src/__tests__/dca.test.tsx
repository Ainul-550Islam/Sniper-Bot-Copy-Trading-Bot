// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tenantRequest = vi.fn();

vi.mock("@/lib/customer-trading-api", () => ({
  tenantRequest: (...args: unknown[]) => tenantRequest(...args),
  classifyTradingError: (error: unknown) => ({
    kind: "error",
    error: String(error),
    retryable: false,
  }),
  isRetryable: () => false,
  tradingStateMessage: (state: { kind: string }) => `state:${state.kind}`,
}));

vi.mock("@/components/AppShell", () => ({
  AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => ({
      organizations: [{ organization_id: "org-1", role: "owner" }],
      selectedOrganizationId: "org-1",
    }),
    subscribe: () => () => {},
  },
}));

import DcaPage from "../app/trading/dca/page";
import { CancelConfirm } from "../components/dca/CancelConfirm";
import { RunsPanel } from "../components/dca/RunsPanel";
import { ScheduleForm } from "../components/dca/ScheduleForm";
import { ScheduleTable } from "../components/dca/ScheduleTable";
import { dcaApi, type DcaSchedule } from "../lib/api/dca-api";

const schedule: DcaSchedule = {
  id: "dca_123",
  mint: "So11111111111111111111111111111111111111112",
  amount_per_run_sol: 0.25,
  interval_secs: 3600,
  budget_sol: 5,
  spent_sol: 1,
  remaining_sol: 4,
  status: "active",
  next_run_at: "2026-10-25T10:00:00Z",
  last_run_at: "2026-10-25T09:00:00Z",
  attempts: 4,
  last_error: null,
  created_at: "2026-10-25T08:00:00Z",
};

describe("DCA schedules", () => {
  beforeEach(() => {
    tenantRequest.mockReset();
  });
  afterEach(() => cleanup());

  it("calls the tenant endpoints with the server create shape", async () => {
    tenantRequest.mockResolvedValue({});
    await dcaApi.list({ status: "active", limit: 25 });
    await dcaApi.create({
      mint: schedule.mint,
      amount_per_run_sol: 0.25,
      interval_secs: 3600,
      budget_sol: 5,
    });
    await dcaApi.pause(schedule.id);
    await dcaApi.resume(schedule.id);
    await dcaApi.cancel(schedule.id);
    await dcaApi.runs(schedule.id);

    expect(tenantRequest.mock.calls).toEqual([
      ["/api/tenant/dca-schedules?status=active&limit=25"],
      [
        "/api/tenant/dca-schedules",
        {
          method: "POST",
          body: {
            mint: schedule.mint,
            amount_per_run_sol: 0.25,
            interval_secs: 3600,
            budget_sol: 5,
          },
        },
      ],
      ["/api/tenant/dca-schedules/dca_123/pause", { method: "POST" }],
      ["/api/tenant/dca-schedules/dca_123/resume", { method: "POST" }],
      ["/api/tenant/dca-schedules/dca_123/cancel", { method: "POST" }],
      ["/api/tenant/dca-schedules/dca_123/runs"],
    ]);
  });

  it("page loads schedules, refetches after pause, and loads run history", async () => {
    let current = { ...schedule };
    tenantRequest.mockImplementation(async (url: string, options?: { method?: string }) => {
      if (url === "/api/tenant/dca-schedules" && !options?.method) {
        return { organization_id: "org-1", schedules: [current] };
      }
      if (url === `/api/tenant/dca-schedules/${schedule.id}/pause`) {
        current = { ...current, status: "paused" };
        return { id: schedule.id, status: "paused" };
      }
      if (url === `/api/tenant/dca-schedules/${schedule.id}/runs`) {
        return {
          schedule_id: schedule.id,
          runs: [
            {
              run_id: 1,
              slot_at: "2026-10-25T09:00:00Z",
              amount_sol: 0.25,
              status: "intended",
              created_at: "2026-10-25T09:00:00Z",
            },
          ],
        };
      }
      throw new Error(`Unexpected request: ${url}`);
    });

    render(<DcaPage />);
    expect(await screen.findByText(schedule.mint)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(await screen.findByText("paused")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Runs" }));
    expect(await screen.findByText("intended")).toBeTruthy();
    expect(tenantRequest.mock.calls.map(([url]) => url)).toEqual([
      "/api/tenant/dca-schedules",
      `/api/tenant/dca-schedules/${schedule.id}/pause`,
      "/api/tenant/dca-schedules",
      `/api/tenant/dca-schedules/${schedule.id}/runs`,
    ]);
  });

  it("page does not call cancel until cancellation is confirmed", async () => {
    let current = { ...schedule };
    tenantRequest.mockImplementation(async (url: string, options?: { method?: string }) => {
      if (url === "/api/tenant/dca-schedules" && !options?.method) {
        return { organization_id: "org-1", schedules: [current] };
      }
      if (url === `/api/tenant/dca-schedules/${schedule.id}/cancel`) {
        current = { ...current, status: "cancelled" };
        return { id: schedule.id, status: "cancelled" };
      }
      throw new Error(`Unexpected request: ${url}`);
    });

    render(<DcaPage />);
    expect(await screen.findByText(schedule.mint)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(tenantRequest.mock.calls.map(([url]) => url)).toEqual([
      "/api/tenant/dca-schedules",
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Confirm cancellation" }));
    expect(await screen.findByText("cancelled")).toBeTruthy();
    expect(tenantRequest.mock.calls.map(([url]) => url)).toEqual([
      "/api/tenant/dca-schedules",
      `/api/tenant/dca-schedules/${schedule.id}/cancel`,
      "/api/tenant/dca-schedules",
    ]);
  });

  it("page shows an error state when the schedule list cannot load", async () => {
    tenantRequest.mockRejectedValue(new Error("storage unavailable"));
    render(<DcaPage />);
    expect(await screen.findByText("state:error")).toBeTruthy();
  });

  it("renders server schedules and forwards pause and runs actions", () => {
    const onPause = vi.fn();
    const onShowRuns = vi.fn();
    render(
      <ScheduleTable
        schedules={[schedule]}
        canManage
        busyScheduleId={null}
        onPause={onPause}
        onResume={() => {}}
        onRequestCancel={() => {}}
        onShowRuns={onShowRuns}
      />,
    );

    expect(screen.getByText(schedule.mint)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    fireEvent.click(screen.getByRole("button", { name: "Runs" }));
    expect(onPause).toHaveBeenCalledWith(schedule);
    expect(onShowRuns).toHaveBeenCalledWith(schedule);
  });

  it("creates with numeric values parsed from the form", async () => {
    const onCreate = vi.fn().mockResolvedValue(undefined);
    render(<ScheduleForm canManage busy={false} onCreate={onCreate} />);

    fireEvent.change(screen.getByLabelText("Token mint"), { target: { value: schedule.mint } });
    fireEvent.change(screen.getByLabelText("Amount per run (SOL)"), { target: { value: "0.25" } });
    fireEvent.change(screen.getByLabelText("Interval (seconds)"), { target: { value: "3600" } });
    fireEvent.change(screen.getByLabelText("Total budget (SOL)"), { target: { value: "5" } });
    fireEvent.click(screen.getByRole("button", { name: "Create DCA schedule" }));

    await waitFor(() =>
      expect(onCreate).toHaveBeenCalledWith({
        mint: schedule.mint,
        amount_per_run_sol: 0.25,
        interval_secs: 3600,
        budget_sol: 5,
      }),
    );
  });

  it("does not cancel until the confirmation button is used", () => {
    const onConfirm = vi.fn();
    const onRequestCancel = vi.fn();
    const { rerender } = render(
      <ScheduleTable
        schedules={[schedule]}
        canManage
        busyScheduleId={null}
        onPause={() => {}}
        onResume={() => {}}
        onRequestCancel={onRequestCancel}
        onShowRuns={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onRequestCancel).toHaveBeenCalledWith(schedule);
    expect(onConfirm).not.toHaveBeenCalled();

    rerender(<CancelConfirm schedule={schedule} busy={false} onConfirm={onConfirm} onDismiss={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Keep schedule" }));
    expect(onConfirm).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Confirm cancellation" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("shows an honest runs error", () => {
    render(
      <RunsPanel
        schedule={schedule}
        runs={null}
        loading={false}
        error="dca_storage_unavailable: storage unavailable"
        onClose={() => {}}
      />,
    );
    expect(screen.getByRole("alert").textContent).toContain("dca_storage_unavailable");
  });
});
