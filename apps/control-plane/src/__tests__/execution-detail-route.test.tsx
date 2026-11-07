// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ executionId: "exec-a", getExecution: vi.fn() }));
vi.mock("next/navigation", () => ({ useParams: () => ({ executionId: mocks.executionId }) }));
vi.mock("@/lib/api/trading-detail-api", () => ({
  tradingDetailApi: { execution: (...args: unknown[]) => mocks.getExecution(...args) },
}));
vi.mock("@/lib/api", () => ({
  ApiError: class ApiError extends Error { status: number; constructor(status: number, message: string) { super(message); this.status = status; } },
  toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error),
}));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/common/ErrorState", () => ({ ErrorState: ({ error }: { error: string }) => <div role="alert">{error}</div> }));
vi.mock("@/components/trading/ExecutionDetail", () => ({ ExecutionDetail: ({ execution }: { execution: { id: string } }) => <div>Execution data {execution.id}</div> }));

import ExecutionDetailPage from "../app/trading/executions/[executionId]/page";

const record = (id: string) => ({ id, status: "filled", amount: "1", price: "10" });

describe("execution detail route", () => {
  beforeEach(() => {
    mocks.executionId = "exec-a";
    mocks.getExecution.mockReset();
  });
  afterEach(() => cleanup());

  it("shows an error and exits loading when the route identifier is missing", async () => {
    mocks.executionId = "";
    render(<ExecutionDetailPage />);
    expect((await screen.findByRole("alert")).textContent).toContain("Execution identifier is missing.");
    expect(screen.queryByText("Loading execution…")).toBeNull();
    expect(mocks.getExecution).not.toHaveBeenCalled();
  });

  it("ignores responses from an old route identifier", async () => {
    let resolveOld!: (value: ReturnType<typeof record>) => void;
    mocks.getExecution
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce(record("exec-b"));
    const view = render(<ExecutionDetailPage />);
    await waitFor(() => expect(mocks.getExecution).toHaveBeenCalledWith("exec-a"));

    mocks.executionId = "exec-b";
    view.rerender(<ExecutionDetailPage />);
    expect(await screen.findByText("Execution data exec-b")).toBeTruthy();

    resolveOld(record("exec-a"));
    await waitFor(() => expect(screen.getByText("Execution data exec-b")).toBeTruthy());
    expect(screen.queryByText("Execution data exec-a")).toBeNull();
  });
});
