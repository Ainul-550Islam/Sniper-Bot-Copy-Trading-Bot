// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ listStrategies: vi.fn() }));
vi.mock("@/lib/api/strategy-api", () => ({ listStrategies: (...args: unknown[]) => mocks.listStrategies(...args) }));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/strategy/strategy-card", () => ({
  default: ({ strategy }: { strategy: { name: string } }) => <div>Strategy: {strategy.name}</div>,
}));
vi.mock("@/components/strategy/strategy-form", () => ({ default: () => <div>Strategy form</div> }));

import StrategiesPage from "../app/strategies/page";

const strategy = (name: string) => ({ id: name, name, module: "copy", parameters: {}, status: "active" });

describe("strategy list filters", () => {
  beforeEach(() => mocks.listStrategies.mockReset());
  afterEach(() => cleanup());

  it("does not let a late result for an old filter replace the selected filter", async () => {
    let resolveAll!: (value: ReturnType<typeof strategy>[]) => void;
    mocks.listStrategies
      .mockReturnValueOnce(new Promise((resolve) => { resolveAll = resolve; }))
      .mockResolvedValueOnce([strategy("copy strategy")]);
    render(<StrategiesPage />);
    await waitFor(() => expect(mocks.listStrategies).toHaveBeenCalledWith(undefined));

    fireEvent.click(screen.getByRole("button", { name: "Copy Trading" }));
    expect(await screen.findByText("Strategy: copy strategy")).toBeTruthy();
    expect(mocks.listStrategies).toHaveBeenLastCalledWith("copy");

    resolveAll([strategy("stale all-modules strategy")]);
    await waitFor(() => expect(screen.getByText("Strategy: copy strategy")).toBeTruthy());
    expect(screen.queryByText("Strategy: stale all-modules strategy")).toBeNull();
  });
});
