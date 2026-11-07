// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ acknowledgeAlert: vi.fn() }));
vi.mock("@/lib/api/alerts-api", () => ({ acknowledgeAlert: (...args: unknown[]) => mocks.acknowledgeAlert(...args) }));

import { AlertCenter } from "../components/alerts/AlertCenter";

const alerts = [{
  id: "alert-1",
  organization_id: "org-1",
  severity: "warning" as const,
  category: "system" as const,
  title: "Worker delayed",
  message: "The worker missed a heartbeat.",
  is_acknowledged: false,
  created_at: "2026-11-03T00:00:00Z",
}];

describe("AlertCenter acknowledgement", () => {
  beforeEach(() => mocks.acknowledgeAlert.mockReset());
  afterEach(() => cleanup());

  it("displays acknowledgement failures and allows retry", async () => {
    const onRefresh = vi.fn();
    mocks.acknowledgeAlert.mockRejectedValueOnce(new Error("network unavailable")).mockResolvedValueOnce({ success: true });
    render(<AlertCenter alerts={alerts} onRefresh={onRefresh} />);

    fireEvent.click(screen.getByRole("button", { name: "Acknowledge" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Could not acknowledge this alert. Please try again.");
    expect(onRefresh).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Acknowledge" }));
    expect(mocks.acknowledgeAlert).toHaveBeenCalledTimes(2);
    await waitFor(() => expect(onRefresh).toHaveBeenCalledTimes(1));
  });
});
