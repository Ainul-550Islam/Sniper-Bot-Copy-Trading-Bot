// @vitest-environment jsdom
import { StrictMode } from "react";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ token: "one-time-token", request: vi.fn() }));
vi.mock("next/navigation", () => ({ useSearchParams: () => new URLSearchParams(mocks.token ? `token=${mocks.token}` : "") }));
vi.mock("next/link", () => ({ default: ({ href, children }: { href: string; children: React.ReactNode }) => <a href={href}>{children}</a> }));
vi.mock("@/lib/api", () => ({
  ApiError: class ApiError extends Error { reason: string; constructor(reason: string) { super(reason); this.reason = reason; } },
  request: (...args: unknown[]) => mocks.request(...args),
}));

import VerifyEmailPage from "../app/verify-email/page";

describe("verify email Strict Mode behavior", () => {
  beforeEach(() => {
    mocks.token = "one-time-token";
    mocks.request.mockReset().mockResolvedValue({ status: "verified", message: "Your email is verified." });
  });
  afterEach(() => cleanup());

  it("sends a one-time confirmation only once despite effect replay", async () => {
    render(<StrictMode><VerifyEmailPage /></StrictMode>);
    expect(await screen.findByText("Your email is verified.")).toBeTruthy();
    expect(mocks.request).toHaveBeenCalledTimes(1);
    expect(mocks.request).toHaveBeenCalledWith("/api/saas/email-verification/confirm", {
      method: "POST",
      body: { token: "one-time-token" },
      anonymous: true,
    });
  });
});
