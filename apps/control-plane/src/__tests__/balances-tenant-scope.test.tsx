// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  balances: vi.fn(),
  listeners: new Set<() => void>(),
}));

vi.mock("@/lib/auth", () => ({
  sessionStore: {
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
}));
vi.mock("@/lib/api", () => ({ toDisplayError: (error: unknown) => String(error) }));
vi.mock("@/lib/api/trading-detail-api", () => ({
  tradingDetailApi: { balances: (...args: unknown[]) => mocks.balances(...args) },
}));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/trading/BalancesTable", () => ({
  BalancesTable: ({ balances }: { balances: Array<{ asset: string }> }) => (
    <div>{balances.map((balance) => <span key={balance.asset}>{balance.asset}</span>)}</div>
  ),
}));

import BalancesPage from "../app/trading/balances/page";

describe("tenant balances scope", () => {
  beforeEach(() => {
    mocks.balances.mockReset();
    mocks.listeners.clear();
    mocks.balances.mockResolvedValue({ items: [{ asset: "ORG-A-ASSET" }] });
  });
  afterEach(() => cleanup());

  it("clears and reloads balances after the selected tenant changes", async () => {
    render(<BalancesPage />);
    expect(await screen.findByText("ORG-A-ASSET")).toBeTruthy();
    mocks.balances.mockResolvedValue({ items: [{ asset: "ORG-B-ASSET" }] });

    mocks.listeners.forEach((listener) => listener());

    expect(await screen.findByText("ORG-B-ASSET")).toBeTruthy();
    await waitFor(() => expect(mocks.balances).toHaveBeenCalledTimes(2));
    expect(screen.queryByText("ORG-A-ASSET")).toBeNull();
  });
});
