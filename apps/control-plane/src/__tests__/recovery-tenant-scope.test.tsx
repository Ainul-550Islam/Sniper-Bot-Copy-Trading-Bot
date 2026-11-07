// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  selectedOrganizationId: "org-a",
  listeners: new Set<() => void>(),
  intents: vi.fn(),
  sweeps: vi.fn(),
}));
vi.mock("@/lib/auth", () => ({
  sessionStore: {
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
}));
vi.mock("@/lib/api", () => ({ toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error) }));
vi.mock("@/lib/api/trading-detail-api", () => ({
  tradingDetailApi: {
    recoveryIntents: (...args: unknown[]) => mocks.intents(...args),
    recoverySweep: (...args: unknown[]) => mocks.sweeps(...args),
  },
}));
vi.mock("@/components/AppShell", () => ({ default: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/common/ErrorState", () => ({ ErrorState: ({ error }: { error: string }) => <div role="alert">{error}</div> }));
vi.mock("@/components/trading/RecoveryPanel", () => ({
  RecoveryPanel: ({ intents, sweeps }: { intents: Array<{ id: string }>; sweeps: Array<{ id: string }> }) => (
    <div>{[...intents, ...sweeps].map((row) => <span key={row.id}>{row.id}</span>)}</div>
  ),
}));

import RecoveryPage from "../app/trading/recovery/page";

describe("recovery tenant scope", () => {
  beforeEach(() => {
    mocks.selectedOrganizationId = "org-a";
    mocks.listeners.clear();
    mocks.intents.mockReset().mockImplementation(() => Promise.resolve({ items: [{ id: mocks.selectedOrganizationId === "org-a" ? "intent-alpha" : "intent-beta" }] }));
    mocks.sweeps.mockReset().mockImplementation(() => Promise.resolve({ items: [{ id: mocks.selectedOrganizationId === "org-a" ? "sweep-alpha" : "sweep-beta" }] }));
  });
  afterEach(() => cleanup());

  it("clears and reloads tenant recovery records after organization selection changes", async () => {
    render(<RecoveryPage />);
    expect(await screen.findByText("intent-alpha")).toBeTruthy();
    expect(screen.getByText("sweep-alpha")).toBeTruthy();

    mocks.selectedOrganizationId = "org-b";
    mocks.listeners.forEach((listener) => listener());
    await waitFor(() => {
      expect(screen.getByText("intent-beta")).toBeTruthy();
      expect(screen.getByText("sweep-beta")).toBeTruthy();
    });
    expect(screen.queryByText("intent-alpha")).toBeNull();
    expect(screen.queryByText("sweep-alpha")).toBeNull();
    expect(mocks.intents).toHaveBeenCalledTimes(2);
  });
});
