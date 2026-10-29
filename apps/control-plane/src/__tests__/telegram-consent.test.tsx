// @vitest-environment jsdom
/**
 * Telegram trade-consent admin surface (Batch 07). The API is mocked; the
 * page must render the live consent from the server (never invented),
 * expose revoke — and only revoke — and refuse a trading link that has no
 * identified Telegram user before anything is sent.
 */
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  telegramStatus: vi.fn(),
  telegramConsent: vi.fn(),
  bindTelegram: vi.fn(),
  unbindTelegram: vi.fn(),
  revokeTelegramConsent: vi.fn(),
}));

vi.mock("@/lib/customer-trading-api", () => ({
  customerTrading: {
    telegramStatus: mocks.telegramStatus,
    telegramConsent: mocks.telegramConsent,
    bindTelegram: mocks.bindTelegram,
    unbindTelegram: mocks.unbindTelegram,
    revokeTelegramConsent: mocks.revokeTelegramConsent,
  },
  classifyTradingError: (e: unknown) => ({ kind: "network", message: String(e) }),
  tradingStateMessage: (s: { message?: string }) => s.message ?? "error",
  isRetryable: () => false,
}));

vi.mock("@/components/AppShell", () => ({
  AppShell: ({ children }: { children: React.ReactNode }) => <main>{children}</main>,
}));

import TelegramPage from "@/app/trading/telegram/page";

const statusWithBinding = {
  organization_id: "org-1",
  module: "telegram",
  entitlement: { feature: null, granted: true },
  effective_state: "enabled",
  runtime: {
    runtime_id: "rt-1",
    generation: "1",
    phase: "running",
    since: "2026-10-29T00:00:00Z",
  },
  runtime_detail: null,
  binding: {
    chat_id: -99,
    bound_at: "2026-10-29T00:00:00Z",
    bound_by: "operator",
    telegram_user_id: 777,
    trading_enabled: true,
  },
  binding_detail: { purpose: "p", delivery: "d" },
  controls: { available: true, actions: ["bind", "unbind", "consent_view", "consent_revoke"] },
};

const activeConsent = {
  organization_id: "org-1",
  chat_id: -99,
  consent: {
    consent_id: "consent-1",
    chat_id: -99,
    user_id: 777,
    scope: ["buy", "sell"],
    max_order_lamports: 500_000_000,
    daily_cap_lamports: 2_000_000_000,
    status: "active",
    granted_at: "2026-10-29T00:00:00Z",
    expires_at: "2026-10-30T00:00:00Z",
    revoked_at: null,
  },
  grant: "in-chat only: use /consent in the bound Telegram chat",
};

describe("Telegram consent admin surface", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.telegramStatus.mockResolvedValue(statusWithBinding);
    mocks.telegramConsent.mockResolvedValue(activeConsent);
  });

  afterEach(() => {
    cleanup();
  });

  it("renders the live consent with caps and a revoke action", async () => {
    render(<TelegramPage />);
    expect(await screen.findByText("Telegram Trade Consent")).toBeTruthy();
    expect(await screen.findByText("active")).toBeTruthy();
    // 500,000,000 lamports = 0.5 SOL — the server number, rendered as-is.
    expect(screen.getByText(/500,000,000 lamports \(0\.5 SOL\)/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Revoke consent" })).toBeTruthy();
    // Granting is in-chat only: the page must say so and offer no grant.
    expect(screen.getByText(/can only view and revoke/i)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /grant/i })).toBeNull();
  });

  it("revokes through the API and reports cancelled pending commands", async () => {
    mocks.revokeTelegramConsent.mockResolvedValue({
      organization_id: "org-1",
      chat_id: -99,
      revoked: { ...activeConsent.consent, status: "revoked", revoked_at: "2026-10-29T01:00:00Z" },
      cancelled_pending: ["cmd-1", "cmd-2"],
    });
    render(<TelegramPage />);
    fireEvent.click(await screen.findByRole("button", { name: "Revoke consent" }));
    expect(
      await screen.findByText(/consent revoked; 2 pending command\(s\) cancelled/i),
    ).toBeTruthy();
    expect(mocks.revokeTelegramConsent).toHaveBeenCalledTimes(1);
  });

  it("shows the in-chat grant path when no consent is live", async () => {
    mocks.telegramConsent.mockResolvedValue({
      ...activeConsent,
      consent: null,
    });
    render(<TelegramPage />);
    expect(await screen.findByText(/No live consent for chat/i)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Revoke consent" })).toBeNull();
  });

  it("refuses a trading link without an identified user before calling the API", async () => {
    mocks.telegramStatus.mockResolvedValue({
      ...statusWithBinding,
      binding: null,
    });
    render(<TelegramPage />);
    const chatInput = await screen.findByLabelText(/Telegram Chat ID/i);
    fireEvent.change(chatInput, { target: { value: "-99" } });
    fireEvent.click(screen.getByLabelText(/Allow the trade console/i));
    fireEvent.click(screen.getByRole("button", { name: "Save Chat Binding" }));
    expect(
      await screen.findByText(/Trading requires a Telegram user id/i),
    ).toBeTruthy();
    expect(mocks.bindTelegram).not.toHaveBeenCalled();
  });

  it("sends the trading link fields explicitly on bind", async () => {
    mocks.telegramStatus.mockResolvedValue({
      ...statusWithBinding,
      binding: null,
    });
    mocks.bindTelegram.mockResolvedValue({
      organization_id: "org-1",
      binding: statusWithBinding.binding,
      detail: "bound",
    });
    render(<TelegramPage />);
    fireEvent.change(await screen.findByLabelText(/Telegram Chat ID/i), {
      target: { value: "-99" },
    });
    fireEvent.change(screen.getByLabelText(/Telegram User ID/i), {
      target: { value: "777" },
    });
    fireEvent.click(screen.getByLabelText(/Allow the trade console/i));
    fireEvent.click(screen.getByRole("button", { name: "Save Chat Binding" }));
    await waitFor(() =>
      expect(mocks.bindTelegram).toHaveBeenCalledWith(-99, {
        telegramUserId: 777,
        tradingEnabled: true,
      }),
    );
  });
});
