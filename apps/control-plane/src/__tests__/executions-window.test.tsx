// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ executionsWindow: vi.fn() }));
vi.mock("@/lib/customer-trading-api", () => ({
  customerTrading: { executionsWindow: (...args: unknown[]) => mocks.executionsWindow(...args) },
  classifyTradingError: () => ({ kind: "error", retryable: false }),
  isRetryable: () => false,
  tradingStateMessage: () => "Unavailable",
}));
vi.mock("@/components/AppShell", () => ({ AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("next/link", () => ({ default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a> }));

import ExecutionsPage from "../app/trading/executions/page";

const response = {
  items: [{
    id: "execution-1",
    at: "2026-11-03T11:00:00Z",
    order_id: "order-12345678",
    kind: "fill",
    status: "confirmed",
    qty: 1.25,
    price: 42.5,
    signature: null,
    error: null,
  }],
  next_cursor: null,
};

describe("execution journal UTC window", () => {
  beforeEach(() => mocks.executionsWindow.mockReset().mockResolvedValue(response));
  afterEach(() => cleanup());

  it("queries only on submit, interprets the datetime-local values as UTC, and renders a matching details column", async () => {
    render(<ExecutionsPage />);
    const sinceInput = screen.getByLabelText("Since (UTC)") as HTMLInputElement;
    const untilInput = screen.getByLabelText("Until (UTC)") as HTMLInputElement;
    await waitFor(() => expect(mocks.executionsWindow).toHaveBeenCalledTimes(1));
    expect(mocks.executionsWindow).toHaveBeenLastCalledWith(
      new Date(`${sinceInput.value}:00Z`).toISOString(),
      new Date(`${untilInput.value}:00Z`).toISOString(),
    );

    fireEvent.change(sinceInput, { target: { value: "2026-11-02T08:00" } });
    fireEvent.change(untilInput, { target: { value: "2026-11-02T09:00" } });
    expect(mocks.executionsWindow).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Query Executions" }));
    await waitFor(() => expect(mocks.executionsWindow).toHaveBeenCalledTimes(2));
    expect(mocks.executionsWindow).toHaveBeenLastCalledWith(
      "2026-11-02T08:00:00.000Z",
      "2026-11-02T09:00:00.000Z",
    );

    const detailsLink = await screen.findByRole("link", { name: "Details" });
    expect(detailsLink.getAttribute("href")).toBe("/trading/executions/execution-1");
    const headers = screen.getAllByRole("columnheader");
    const cells = screen.getAllByRole("cell");
    expect(headers).toHaveLength(9);
    expect(cells).toHaveLength(9);
  });

  it("rejects an invalid UTC range without making another request", async () => {
    render(<ExecutionsPage />);
    await waitFor(() => expect(mocks.executionsWindow).toHaveBeenCalledTimes(1));
    fireEvent.change(screen.getByLabelText("Since (UTC)"), { target: { value: "2026-11-03T10:00" } });
    fireEvent.change(screen.getByLabelText("Until (UTC)"), { target: { value: "2026-11-03T09:00" } });
    fireEvent.click(screen.getByRole("button", { name: "Query Executions" }));
    expect(await screen.findByText(/Since no later than Until/)).toBeTruthy();
    expect(mocks.executionsWindow).toHaveBeenCalledTimes(1);
  });
});
