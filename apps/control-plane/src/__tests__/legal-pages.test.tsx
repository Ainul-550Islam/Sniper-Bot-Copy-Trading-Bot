// @vitest-environment jsdom
import type { ComponentType } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import TermsPage from "@/app/legal/terms/page";
import PrivacyPage from "@/app/legal/privacy/page";
import RiskDisclosurePage from "@/app/legal/risk-disclosure/page";
import HomePage from "@/app/page";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn(), back: vi.fn(), prefetch: vi.fn() }),
  usePathname: () => "/",
  useSearchParams: () => new URLSearchParams(),
}));

const legalPages: Array<{ name: string; Page: ComponentType; heading: string; disclosure: RegExp }> = [
  { name: "terms", Page: TermsPage, heading: "Terms of Service", disclosure: /No guarantee of results/ },
  { name: "privacy", Page: PrivacyPage, heading: "Privacy Policy", disclosure: /Session tokens are held in browser memory only/ },
  { name: "risk disclosure", Page: RiskDisclosurePage, heading: "Risk Disclosure", disclosure: /substantial risk of loss/ },
];

afterEach(() => cleanup());

describe("legal pages and sign-up consent", () => {
  it.each(legalPages)("renders $name content", ({ Page, heading, disclosure }) => {
    render(<Page />);
    expect(screen.getByRole("heading", { level: 1, name: heading })).toBeTruthy();
    expect(screen.getByText(disclosure)).toBeTruthy();
  });

  it("keeps consent document links valid and records checkbox state through the parent form", () => {
    render(<HomePage />);
    fireEvent.click(screen.getByRole("tab", { name: "Create account" }));
    const links = [
      ["Terms of Service", "/legal/terms"],
      ["Privacy Policy", "/legal/privacy"],
      ["Risk Disclosure", "/legal/risk-disclosure"],
    ] as const;
    for (const [label, href] of links) {
      expect(screen.getByRole("link", { name: label }).getAttribute("href")).toBe(href);
    }
    const checkbox = screen.getByRole("checkbox");
    expect((checkbox as HTMLInputElement).checked).toBe(false);
    const consentText = screen.getByText(/Acceptance is recorded with document version/);
    expect(consentText.textContent).not.toContain(" at ");

    fireEvent.click(checkbox);
    expect((checkbox as HTMLInputElement).checked).toBe(true);
    expect(screen.getByText(/Acceptance is recorded with document version/).textContent).toContain(" at ");

    fireEvent.click(checkbox);
    expect((checkbox as HTMLInputElement).checked).toBe(false);
    expect(screen.getByText(/Acceptance is recorded with document version/).textContent).not.toContain(" at ");
  });
});
