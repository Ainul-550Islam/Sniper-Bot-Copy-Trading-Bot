// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  selectedOrganizationId: "org-a",
  listeners: new Set<() => void>(),
  list: vi.fn(),
}));

vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => ({
      selectedOrganizationId: mocks.selectedOrganizationId,
      organizations: [
        { organization_id: "org-a", role: "owner" },
        { organization_id: "org-b", role: "viewer" },
      ],
    }),
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
}));
vi.mock("@/lib/api/dca-api", () => ({
  dcaApi: {
    list: (...args: unknown[]) => mocks.list(...args),
    create: vi.fn(), pause: vi.fn(), resume: vi.fn(), cancel: vi.fn(), runs: vi.fn(),
  },
}));
vi.mock("@/lib/customer-trading-api", () => ({
  classifyTradingError: () => ({ kind: "error", retryable: false }),
  isRetryable: () => false,
  tradingStateMessage: () => "Unavailable",
}));
vi.mock("@/components/AppShell", () => ({ AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/dca/ScheduleForm", () => ({
  ScheduleForm: ({ canManage }: { canManage: boolean }) => <div>Can manage DCA: {String(canManage)}</div>,
}));
vi.mock("@/components/dca/ScheduleTable", () => ({
  ScheduleTable: ({ schedules }: { schedules: Array<{ mint: string }> }) => (
    <div>{schedules.map((schedule) => <span key={schedule.mint}>{schedule.mint}</span>)}</div>
  ),
}));
vi.mock("@/components/dca/CancelConfirm", () => ({ CancelConfirm: () => null }));
vi.mock("@/components/dca/RunsPanel", () => ({ RunsPanel: () => null }));

import DcaPage from "../app/trading/dca/page";

const schedulesFor = (mint: string) => [{
  id: `schedule-${mint}`,
  mint,
  amount_per_run_sol: 0.1,
  interval_secs: 60,
  budget_sol: 1,
  spent_sol: 0,
  remaining_sol: 1,
  status: "active",
  next_run_at: null,
  last_run_at: null,
  attempts: 0,
  last_error: null,
  created_at: "2026-10-30T00:00:00Z",
}];

describe("DCA tenant scope", () => {
  beforeEach(() => {
    mocks.selectedOrganizationId = "org-a";
    mocks.listeners.clear();
    mocks.list.mockReset().mockImplementation(() => Promise.resolve({
      schedules: schedulesFor(mocks.selectedOrganizationId === "org-a" ? "alpha-mint" : "beta-mint"),
    }));
  });
  afterEach(() => cleanup());

  it("reloads schedules and updates role controls when the selected tenant changes", async () => {
    render(<DcaPage />);
    expect(await screen.findByText("alpha-mint")).toBeTruthy();
    expect(screen.getByText("Can manage DCA: true")).toBeTruthy();

    mocks.selectedOrganizationId = "org-b";
    mocks.listeners.forEach((listener) => listener());

    await waitFor(() => {
      expect(screen.getByText("beta-mint")).toBeTruthy();
      expect(screen.getByText("Can manage DCA: false")).toBeTruthy();
    });
    expect(screen.queryByText("alpha-mint")).toBeNull();
    expect(mocks.list).toHaveBeenCalledTimes(2);
  });
});
