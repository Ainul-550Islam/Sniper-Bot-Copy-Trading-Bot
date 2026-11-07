// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ executionsForOrder: vi.fn() }));
vi.mock("@/lib/customer-trading-api", () => ({
  customerTrading: { executionsForOrder: (...args: unknown[]) => mocks.executionsForOrder(...args) },
  classifyTradingError: () => ({ kind: "unknown" }),
  isRetryable: () => false,
  tradingStateMessage: () => "Could not load execution records",
}));

import ExecutionStatus from "@/components/trading/ExecutionStatus";

type TestExecution = {
  id: string;
  order_id: string;
  kind: string;
  status: string;
  qty: number | null;
  price: number | null;
  at: string;
  signature: string | null;
  error: string | null;
};
const page = (item: TestExecution) => ({ items: [item] });
const execution = (id: string, kind: string): TestExecution => ({
  id,
  order_id: id,
  kind,
  status: "confirmed",
  qty: 1,
  price: 1,
  at: "2026-01-01T00:00:00Z",
  signature: null,
  error: null,
});

describe("execution status order scope", () => {
  beforeEach(() => mocks.executionsForOrder.mockReset());
  afterEach(() => cleanup());

  it("clears previous order rows and ignores a late response after the order changes", async () => {
    let resolveOld!: (value: ReturnType<typeof page>) => void;
    let resolveNew!: (value: ReturnType<typeof page>) => void;
    mocks.executionsForOrder
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockReturnValueOnce(new Promise((resolve) => { resolveNew = resolve; }));
    const { rerender } = render(<ExecutionStatus orderId="order-a" />);
    await waitFor(() => expect(mocks.executionsForOrder).toHaveBeenCalledWith("order-a"));

    rerender(<ExecutionStatus orderId="order-b" />);
    await waitFor(() => expect(mocks.executionsForOrder).toHaveBeenCalledWith("order-b"));
    expect(screen.getByText("Loading execution records…")).toBeTruthy();
    await act(async () => resolveNew(page(execution("exec-b", "new-order-row"))));
    expect(await screen.findByText("new-order-row")).toBeTruthy();

    await act(async () => resolveOld(page(execution("exec-a", "old-order-row"))));
    expect(screen.getByText("new-order-row")).toBeTruthy();
    expect(screen.queryByText("old-order-row")).toBeNull();
    expect(screen.getByLabelText("Execution status for order order-b")).toBeTruthy();
  });
});
