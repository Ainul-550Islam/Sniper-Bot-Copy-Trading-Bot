// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  selectedOrganizationId: "org-a",
  listeners: new Set<() => void>(),
  status: vi.fn(),
  consent: vi.fn(),
}));
vi.mock("@/lib/auth", () => ({
  sessionStore: {
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
}));
vi.mock("@/lib/customer-trading-api", () => ({
  customerTrading: {
    telegramStatus: (...args: unknown[]) => mocks.status(...args),
    telegramConsent: (...args: unknown[]) => mocks.consent(...args),
    bindTelegram: vi.fn(),
    unbindTelegram: vi.fn(),
    revokeTelegramConsent: vi.fn(),
  },
  classifyTradingError: () => ({ kind: "error", retryable: false }),
  isRetryable: () => false,
  tradingStateMessage: () => "Unavailable",
}));
vi.mock("@/components/AppShell", () => ({ AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</> }));

import TelegramPage from "../app/trading/telegram/page";

function statusFor(org: string) {
  return {
    runtime: { phase: "running", since: "2026-11-03T00:00:00Z", runtime_id: "runtime-1", generation: 1 },
    runtime_detail: null,
    binding: {
      chat_id: org === "org-a" ? "-100111" : "-100222",
      bound_at: "2026-11-03T00:00:00Z",
      bound_by: "operator",
      telegram_user_id: org === "org-a" ? 111 : 222,
      trading_enabled: false,
    },
  };
}

describe("Telegram tenant scope", () => {
  beforeEach(() => {
    mocks.selectedOrganizationId = "org-a";
    mocks.listeners.clear();
    mocks.status.mockReset().mockImplementation(() => Promise.resolve(statusFor(mocks.selectedOrganizationId)));
    mocks.consent.mockReset().mockImplementation(() => Promise.resolve({
      chat_id: mocks.selectedOrganizationId === "org-a" ? "-100111" : "-100222",
      consent: null,
    }));
  });
  afterEach(() => cleanup());

  it("clears prior chat binding and reloads consent after a tenant switch", async () => {
    render(<TelegramPage />);
    const chatInput = screen.getByLabelText("Telegram Chat ID (Numeric)") as HTMLInputElement;
    await waitFor(() => expect(chatInput.value).toBe("-100111"));
    expect(screen.getByText(/No live consent for chat/).textContent).toContain("-100111");

    mocks.selectedOrganizationId = "org-b";
    mocks.listeners.forEach((listener) => listener());
    await waitFor(() => expect(chatInput.value).toBe("-100222"));
    expect(screen.getByText(/No live consent for chat/).textContent).toContain("-100222");
    expect(screen.queryByText("-100111")).toBeNull();
    expect(mocks.status).toHaveBeenCalledTimes(2);
    expect(mocks.consent).toHaveBeenCalledTimes(2);
  });
});
