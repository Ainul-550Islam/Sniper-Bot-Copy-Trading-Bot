// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const request = vi.fn();

vi.mock("@/lib/api", () => ({
  request: (...args: unknown[]) => request(...args),
}));

import { LifecycleActions } from "../components/lifecycle/LifecycleActions";
import { lifecycleActionsApi } from "../lib/api/lifecycle-actions-api";

const organizationId = "550e8400-e29b-41d4-a716-446655440000";

describe("Data lifecycle actions", () => {
  beforeEach(() => request.mockReset());
  afterEach(() => cleanup());

  it("posts each action to its exact server path", async () => {
    request.mockResolvedValue({ ok: true });
    await lifecycleActionsApi.disconnect(organizationId);
    await lifecycleActionsApi.revokeCredentials(organizationId);
    await lifecycleActionsApi.revokeSessions(organizationId);
    await lifecycleActionsApi.scheduleRetention(organizationId, { confirm: true });

    expect(request.mock.calls).toEqual([
      [`/api/saas/data-lifecycle/${organizationId}/disconnect`, { method: "POST" }],
      [`/api/saas/data-lifecycle/${organizationId}/revoke-credentials`, { method: "POST" }],
      [`/api/saas/data-lifecycle/${organizationId}/revoke-sessions`, { method: "POST" }],
      [`/api/saas/data-lifecycle/${organizationId}/schedule-retention`, { method: "POST", body: { confirm: true } }],
    ]);
  });

  it("does not invoke an action before its typed confirmation matches", () => {
    const onAction = vi.fn();
    render(
      <LifecycleActions
        organizationId={organizationId}
        canManage
        busyAction={null}
        onAction={onAction}
      />,
    );

    const disconnectButton = screen.getByRole("button", { name: "Disconnect sockets" }) as HTMLButtonElement;
    expect(disconnectButton.disabled).toBe(true);
    fireEvent.click(disconnectButton);
    expect(onAction).not.toHaveBeenCalled();

    fireEvent.change(screen.getByLabelText(/Type DISCONNECT to confirm/), {
      target: { value: "DISCONNECT" },
    });
    expect(disconnectButton.disabled).toBe(false);
    fireEvent.click(disconnectButton);
    expect(onAction).toHaveBeenCalledWith("disconnect");
  });

  it("uses a separate typed confirmation for every destructive action", () => {
    const onAction = vi.fn();
    render(
      <LifecycleActions
        organizationId={organizationId}
        canManage
        busyAction={null}
        onAction={onAction}
      />,
    );

    const cases = [
      ["REVOKE", "Revoke credentials", "revoke-credentials"],
      ["SESSIONS", "Revoke sessions", "revoke-sessions"],
      ["RETENTION", "Schedule retention", "schedule-retention"],
    ] as const;

    for (const [word, button, action] of cases) {
      const control = screen.getByRole("button", { name: button }) as HTMLButtonElement;
      expect(control.disabled).toBe(true);
      fireEvent.change(screen.getByLabelText(new RegExp(`Type ${word} to confirm`)), {
        target: { value: word },
      });
      fireEvent.click(control);
      expect(onAction).toHaveBeenCalledWith(action);
    }
    expect(onAction).toHaveBeenCalledTimes(3);
  });

  it("disables all typed actions for a read-only role", () => {
    render(
      <LifecycleActions
        organizationId={organizationId}
        canManage={false}
        busyAction={null}
        onAction={() => {}}
      />,
    );
    expect((screen.getByRole("button", { name: "Revoke credentials" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByLabelText(/Type REVOKE to confirm/) as HTMLInputElement).disabled).toBe(true);
  });
});
