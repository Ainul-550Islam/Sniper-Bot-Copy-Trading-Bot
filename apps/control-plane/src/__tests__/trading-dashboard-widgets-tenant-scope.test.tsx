// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  snapshot: { selectedOrganizationId: "org-a" },
  listeners: new Set<() => void>(),
  pnlToday: vi.fn(),
  bots: vi.fn(),
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
  customerTrading: {
    pnlToday: () => mocks.pnlToday(),
    bots: () => mocks.bots(),
  },
  classifyTradingError: (error: unknown) => ({ kind: "error", reason: error instanceof Error ? error.message : "failed" }),
  isRetryable: () => false,
  tradingStateMessage: (state: { reason?: string }) => state.reason ?? "Loading",
}));

import PnlCard from "@/components/trading/PnlCard";
import RuntimeCard from "@/components/trading/RuntimeCard";

const pnl = (organizationId: string, value: number) => ({ organization_id: organizationId, pnl: value });
const bots = (organizationId: string, phase: string) => ({
  organization_id: organizationId,
  count: 1,
  items: [{
    module: "sniper",
    runtime_id: `${organizationId}-runtime-id`,
    generation: "generation-1234",
    phase,
    since: "2026-01-01T00:00:00Z",
    controls: "tenant",
  }],
});

describe("trading dashboard widget tenant scope", () => {
  beforeEach(() => {
    mocks.snapshot = { selectedOrganizationId: "org-a" };
    mocks.listeners.clear();
    mocks.pnlToday.mockReset();
    mocks.bots.mockReset();
  });
  afterEach(() => cleanup());

  it("clears PnL and ignores a late response from the previous organization", async () => {
    let resolveOld!: (value: ReturnType<typeof pnl>) => void;
    mocks.pnlToday
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce(pnl("org-b", 2));
    render(<PnlCard />);
    await waitFor(() => expect(mocks.pnlToday).toHaveBeenCalledTimes(1));

    act(() => {
      mocks.snapshot = { selectedOrganizationId: "org-b" };
      mocks.listeners.forEach((listener) => listener());
    });
    expect(await screen.findByText("2.0000")).toBeTruthy();
    await act(async () => resolveOld(pnl("org-a", 1)));
    expect(screen.getByText("2.0000")).toBeTruthy();
    expect(screen.queryByText("1.0000")).toBeNull();
  });

  it("clears runtimes and ignores a late response from the previous organization", async () => {
    let resolveOld!: (value: ReturnType<typeof bots>) => void;
    mocks.bots
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce(bots("org-b", "phase-org-b"));
    render(<RuntimeCard />);
    await waitFor(() => expect(mocks.bots).toHaveBeenCalledTimes(1));

    act(() => {
      mocks.snapshot = { selectedOrganizationId: "org-b" };
      mocks.listeners.forEach((listener) => listener());
    });
    expect(await screen.findByText("phase-org-b")).toBeTruthy();
    await act(async () => resolveOld(bots("org-a", "phase-org-a")));
    expect(screen.getByText("phase-org-b")).toBeTruthy();
    expect(screen.queryByText("phase-org-a")).toBeNull();
  });
});
