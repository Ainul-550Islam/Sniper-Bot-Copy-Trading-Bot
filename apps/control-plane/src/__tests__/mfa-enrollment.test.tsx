// @vitest-environment jsdom
import { StrictMode, type ReactNode } from "react";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  setupTotp: vi.fn(),
  verifyTotp: vi.fn(),
  needsMfaEnrollment: vi.fn(),
  refresh: vi.fn(),
  replace: vi.fn(),
}));

vi.mock("@/lib/api/security-api", () => ({
  setupTotp: mocks.setupTotp,
  verifyTotp: mocks.verifyTotp,
}));

vi.mock("@/lib/auth", () => ({
  needsMfaEnrollment: mocks.needsMfaEnrollment,
  refresh: mocks.refresh,
}));

vi.mock("@/lib/api", () => ({
  ApiError: class ApiError extends Error {
    reason: string;
    constructor(message: string, reason: string) {
      super(message);
      this.reason = reason;
    }
  },
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace: mocks.replace }),
}));

vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: ReactNode }) => (
    <a href={href}>{children}</a>
  ),
}));

vi.mock("@/components/settings/TotpQrCode", () => ({
  default: ({ label }: { label: string }) => <div>{label}</div>,
}));

import MfaEnrollmentPage from "@/app/mfa-enrollment/page";

describe("MFA enrollment page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.needsMfaEnrollment.mockReturnValue(true);
    mocks.setupTotp.mockResolvedValue({
      device_id: "device-1",
      secret: "TEST-SECRET-1",
      otpauth_url: "otpauth://totp/example",
      verified: false,
      backup_codes: null,
    });
  });

  afterEach(() => cleanup());

  it("creates only one TOTP device when Strict Mode replays mount effects", async () => {
    render(
      <StrictMode>
        <MfaEnrollmentPage />
      </StrictMode>,
    );

    expect(await screen.findByText("TEST-SECRET-1")).toBeTruthy();
    expect(mocks.setupTotp).toHaveBeenCalledTimes(1);
  });
});
