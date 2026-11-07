// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ getRiskDashboard: vi.fn() }));

vi.mock("@/lib/api/risk-api", () => ({
  getRiskDashboard: mocks.getRiskDashboard,
}));
vi.mock("@/components/AppShell", () => ({
  default: ({ children }: { children: React.ReactNode }) => <main>{children}</main>,
}));
vi.mock("@/components/common/ErrorState", () => ({
  ErrorState: ({ error, onRetry }: { error: string; onRetry: () => void }) => (
    <div role="alert">
      <span>{error}</span>
      <button type="button" onClick={onRetry}>Retry</button>
    </div>
  ),
}));
vi.mock("@/components/risk/KillSwitchPanel", () => ({
  default: ({
    killSwitchActive,
    onRefresh,
  }: {
    killSwitchActive: boolean;
    onRefresh: () => void;
  }) => (
    <div>
      <div>Kill switch active: {String(killSwitchActive)}</div>
      <button type="button" onClick={onRefresh}>Refresh risk</button>
    </div>
  ),
}));
vi.mock("@/components/risk/RiskLimitPanel", () => ({
  default: () => <div>Tenant risk limits</div>,
}));

import RiskPage from "@/app/risk/page";

describe("risk dashboard refresh failure", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.getRiskDashboard
      .mockResolvedValueOnce({
        kill_switch_active: false,
        rules: [],
        reference_asset: "USD",
      })
      .mockRejectedValueOnce(new Error("risk service unavailable"));
  });

  afterEach(() => cleanup());

  it("does not show stale safeguards as current after a failed refresh", async () => {
    render(<RiskPage />);
    expect(await screen.findByText("Kill switch active: false")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Refresh risk" }));
    expect(await screen.findByText("risk service unavailable")).toBeTruthy();
    expect(screen.queryByText("Kill switch active: false")).toBeNull();
    expect(screen.queryByText("Tenant risk limits")).toBeNull();
  });
});
