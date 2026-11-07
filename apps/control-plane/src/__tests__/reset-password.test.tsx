// @vitest-environment jsdom
import type { ReactNode } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  clearSession: vi.fn(),
  push: vi.fn(),
  get: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  request: mocks.request,
  ApiError: class ApiError extends Error {
    reason: string;
    constructor(message: string, reason: string) {
      super(message);
      this.reason = reason;
    }
  },
}));

vi.mock("@/lib/auth", () => ({ clearSession: mocks.clearSession }));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: mocks.push }),
  useSearchParams: () => ({ get: mocks.get }),
}));
vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: ReactNode }) => (
    <a href={href}>{children}</a>
  ),
}));

import ResetPasswordPage from "@/app/reset-password/page";

describe("password reset session teardown", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.get.mockReturnValue("single-use-reset-token");
    mocks.request.mockResolvedValue({
      status: "ok",
      message: "Password updated. Sign in again.",
    });
  });

  afterEach(() => cleanup());

  it("clears the local in-memory session after server-confirmed reset", async () => {
    render(<ResetPasswordPage />);

    fireEvent.change(screen.getByLabelText(/^New password/), {
      target: { value: "a-new-password-with-12-plus" },
    });
    fireEvent.change(screen.getByLabelText(/^Repeat new password/), {
      target: { value: "a-new-password-with-12-plus" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Set new password" }));

    expect(await screen.findByText("Password updated. Sign in again.")).toBeTruthy();
    expect(mocks.request).toHaveBeenCalledWith("/api/saas/password-reset/confirm", {
      method: "POST",
      body: {
        token: "single-use-reset-token",
        new_password: "a-new-password-with-12-plus",
      },
      anonymous: true,
    });
    expect(mocks.clearSession).toHaveBeenCalledWith("expired");
  });
});
