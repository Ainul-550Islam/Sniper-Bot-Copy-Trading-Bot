// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ signature: "signature-a", getTransaction: vi.fn() }));
vi.mock("next/navigation", () => ({ useParams: () => ({ signature: mocks.signature }) }));
vi.mock("@/lib/api/trading-detail-api", () => ({ tradingDetailApi: { transaction: (...args: unknown[]) => mocks.getTransaction(...args) } }));
vi.mock("@/lib/api", () => ({
  ApiError: class ApiError extends Error { status: number; constructor(status: number, message: string) { super(message); this.status = status; } },
  toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error),
}));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/common/ErrorState", () => ({ ErrorState: ({ error }: { error: string }) => <div role="alert">{error}</div> }));
vi.mock("@/components/trading/TransactionDetail", () => ({
  TransactionDetail: ({ transaction }: { transaction: { signature: string } }) => <div>Transaction data {transaction.signature}</div>,
}));

import TransactionDetailPage from "../app/trading/transactions/[signature]/page";

const record = (signature: string) => ({ signature, slot: 1, err: null, block_time: "2026-11-03T00:00:00Z" });

describe("transaction detail route", () => {
  beforeEach(() => {
    mocks.signature = "signature-a";
    mocks.getTransaction.mockReset();
  });
  afterEach(() => cleanup());

  it("shows a terminal error when the signature is missing", async () => {
    mocks.signature = "";
    render(<TransactionDetailPage />);
    expect((await screen.findByRole("alert")).textContent).toContain("Transaction signature is missing.");
    expect(screen.queryByText("Loading transaction…")).toBeNull();
    expect(mocks.getTransaction).not.toHaveBeenCalled();
  });

  it("ignores a late response after navigating to a different signature", async () => {
    let resolveOld!: (value: ReturnType<typeof record>) => void;
    mocks.getTransaction
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce(record("signature-b"));
    const view = render(<TransactionDetailPage />);
    await waitFor(() => expect(mocks.getTransaction).toHaveBeenCalledWith("signature-a"));
    mocks.signature = "signature-b";
    view.rerender(<TransactionDetailPage />);
    expect(await screen.findByText("Transaction data signature-b")).toBeTruthy();
    resolveOld(record("signature-a"));
    await waitFor(() => expect(screen.getByText("Transaction data signature-b")).toBeTruthy());
    expect(screen.queryByText("Transaction data signature-a")).toBeNull();
  });
});
