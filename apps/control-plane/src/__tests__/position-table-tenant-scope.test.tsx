// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  snapshot: { selectedOrganizationId: "org-a" },
  listeners: new Set<() => void>(),
  positions: vi.fn(),
}));
vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => mocks.snapshot,
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
}));
vi.mock("@/lib/customer-trading-api", () => ({
  customerTrading: { positions: (...args: unknown[]) => mocks.positions(...args) },
  classifyTradingError: (error: unknown) => ({ kind: "error", reason: error instanceof Error ? error.message : "failed" }),
  isRetryable: () => false,
  tradingStateMessage: (state: { reason?: string }) => state.reason ?? "Loading",
}));
vi.mock("next/link", () => ({ default: ({ children, href }: { children: React.ReactNode; href: string }) => <a href={href}>{children}</a> }));

import PositionTable from "@/components/trading/PositionTable";

const position = (id: string, symbol: string) => ({
  id,
  symbol,
  side: "long",
  qty: 1,
  entry_price: 2,
  mark_price: 3,
  realized_pnl: 0,
  unrealized_pnl: 1,
  status: "open",
  opened_at: "2026-01-01T00:00:00Z",
  closed_at: null,
});
const page = (organizationId: string, item: ReturnType<typeof position>) => ({
  organization_id: organizationId,
  items: [item],
  next_cursor: null,
});

describe("position table tenant scope", () => {
  beforeEach(() => {
    mocks.snapshot = { selectedOrganizationId: "org-a" };
    mocks.listeners.clear();
    mocks.positions.mockReset();
  });
  afterEach(() => cleanup());

  it("clears positions and ignores a late response after switching organizations", async () => {
    let resolveOld!: (value: ReturnType<typeof page>) => void;
    mocks.positions
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce(page("org-b", position("position-b", "B-COIN")));
    render(<PositionTable />);
    await waitFor(() => expect(mocks.positions).toHaveBeenCalledTimes(1));

    act(() => {
      mocks.snapshot = { selectedOrganizationId: "org-b" };
      mocks.listeners.forEach((listener) => listener());
    });
    expect(await screen.findByText("B-COIN")).toBeTruthy();
    await act(async () => resolveOld(page("org-a", position("position-a", "A-COIN"))));
    expect(screen.getByText("B-COIN")).toBeTruthy();
    expect(screen.queryByText("A-COIN")).toBeNull();
  });

  it("rejects positions returned for a different organization", async () => {
    mocks.positions.mockResolvedValue(page("org-b", position("position-b", "B-COIN")));
    render(<PositionTable />);
    expect(await screen.findByText("Position response does not match the selected organization")).toBeTruthy();
    expect(screen.queryByText("B-COIN")).toBeNull();
  });
});
