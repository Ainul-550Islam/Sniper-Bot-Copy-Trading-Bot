// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ moduleControl: vi.fn() }));
vi.mock("@/lib/customer-trading-api", () => ({
  customerTrading: { moduleControl: (...args: unknown[]) => mocks.moduleControl(...args) },
  classifyTradingError: () => ({ kind: "unknown" }),
  tradingStateMessage: () => "Could not apply module control",
}));

import ModuleActionButton from "@/components/trading/ModuleActionButton";

describe("module action disable reason", () => {
  beforeEach(() => mocks.moduleControl.mockReset().mockResolvedValue({ control_result: { detail: "Disabled" } }));
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it.each([null, "   "]) ("does not disable when the reason is cancelled or blank (%s)", (reason) => {
    vi.spyOn(window, "prompt").mockReturnValue(reason);
    render(<ModuleActionButton module="sniper" action="disable" label="Disable Sniper" available />);
    fireEvent.click(screen.getByRole("button", { name: "Disable Sniper" }));
    expect(mocks.moduleControl).not.toHaveBeenCalled();
  });

  it("trims and sends the required reason", async () => {
    vi.spyOn(window, "prompt").mockReturnValue("  planned maintenance  ");
    const onDone = vi.fn();
    const response = { control_result: { detail: "Disabled" } };
    mocks.moduleControl.mockResolvedValue(response);
    render(<ModuleActionButton module="sniper" action="disable" label="Disable Sniper" available onDone={onDone} />);
    fireEvent.click(screen.getByRole("button", { name: "Disable Sniper" }));

    await waitFor(() => expect(mocks.moduleControl).toHaveBeenCalledWith("sniper", "disable", "planned maintenance"));
    expect(onDone).toHaveBeenCalledWith(response);
    expect(await screen.findByText("Disabled")).toBeTruthy();
  });
});
