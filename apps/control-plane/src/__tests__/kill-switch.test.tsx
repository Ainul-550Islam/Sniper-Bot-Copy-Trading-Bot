// @vitest-environment jsdom
/**
 * Kill-switch panel behaviour (File 3 §7). The API is mocked; the panel must
 * report failures instead of showing a success state, and must not refresh
 * or claim success when the operator cancels.
 */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const toggleKillSwitch = vi.fn();

vi.mock("@/lib/api/risk-api", () => ({
  toggleKillSwitch: (...args: unknown[]) => toggleKillSwitch(...args),
}));

import { KillSwitchPanel } from "../components/risk/KillSwitchPanel";

describe("KillSwitchPanel", () => {
  beforeEach(() => {
    toggleKillSwitch.mockReset();
    vi.spyOn(window, "confirm").mockReturnValue(true);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("shows the blocked state when the kill switch is active", () => {
    render(<KillSwitchPanel killSwitchActive onRefresh={() => {}} />);
    expect(screen.getByText("ACTIVE — TRADING BLOCKED")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Deactivate & Resume Trading" })).toBeTruthy();
  });

  it("reports a failed toggle and does not refresh as if it succeeded", async () => {
    toggleKillSwitch.mockRejectedValue(new Error("network unreachable"));
    const onRefresh = vi.fn();
    render(<KillSwitchPanel killSwitchActive={false} onRefresh={onRefresh} />);

    fireEvent.click(screen.getByRole("button", { name: "ACTIVATE EMERGENCY STOP" }));

    expect(await screen.findByText("network unreachable")).toBeTruthy();
    expect(onRefresh).not.toHaveBeenCalled();
    expect(screen.queryByText(/success/i)).toBeNull();
  });

  it("refreshes only after the server confirms the change", async () => {
    toggleKillSwitch.mockResolvedValue({});
    const onRefresh = vi.fn();
    render(<KillSwitchPanel killSwitchActive={false} onRefresh={onRefresh} />);

    fireEvent.click(screen.getByRole("button", { name: "ACTIVATE EMERGENCY STOP" }));

    await waitFor(() => expect(onRefresh).toHaveBeenCalledTimes(1));
    expect(toggleKillSwitch).toHaveBeenCalledWith({
      active: true,
      reason: "Manual operator emergency trigger",
    });
  });

  it("sends nothing when the operator cancels the confirmation", () => {
    vi.spyOn(window, "confirm").mockReturnValue(false);
    render(<KillSwitchPanel killSwitchActive={false} onRefresh={() => {}} />);

    fireEvent.click(screen.getByRole("button", { name: "ACTIVATE EMERGENCY STOP" }));

    expect(toggleKillSwitch).not.toHaveBeenCalled();
  });
});
