// @vitest-environment jsdom
import type { ReactNode } from "react";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  revoke: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  apiKeys: {
    list: mocks.list,
    create: mocks.create,
    revoke: mocks.revoke,
  },
  toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error),
}));
vi.mock("@/components/AppShell", () => ({
  default: ({ children }: { children: ReactNode }) => <main>{children}</main>,
}));

import ApiKeysSettingsPage from "@/app/settings/api/page";

describe("tenant API key list retry", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.list
      .mockRejectedValueOnce(new Error("temporary network failure"))
      .mockResolvedValueOnce({ keys: [] });
  });

  afterEach(() => cleanup());

  it("offers a safe read retry after a non-authentication API failure", async () => {
    render(<ApiKeysSettingsPage />);

    expect(await screen.findByText("temporary network failure")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Retry loading API keys" }));

    expect(await screen.findByText("No API keys are configured for this organization.")).toBeTruthy();
    expect(mocks.list).toHaveBeenCalledTimes(2);
    expect(mocks.create).not.toHaveBeenCalled();
    expect(mocks.revoke).not.toHaveBeenCalled();
  });
});
