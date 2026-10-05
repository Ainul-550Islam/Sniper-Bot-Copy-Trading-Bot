"use client";

/**
 * Tenant Onboarding & Activation Wizard (SECOND.md §61).
 *
 * Guides new quantitative teams through tenant provisioning, KMS signer setup,
 * subscription tier entitlement, and first bot strategy deployment.
 */

import { useState } from "react";
import AppShell from "@/components/AppShell";
import Link from "next/link";

export default function OnboardingPage() {
  const [currentStep, setCurrentStep] = useState(1);

  const steps = [
    {
      step: 1,
      title: "Organization Provisioning",
      desc: "Tenant namespace and schema initialized with dedicated audit logging.",
      completed: true,
    },
    {
      step: 2,
      title: "Custody Key Allocation",
      desc: "FIPS 140-3 HSM KMS custody profile generated for Ed25519 signing.",
      completed: currentStep > 2,
    },
    {
      step: 3,
      title: "Strategy Configuration",
      desc: "Define your execution rules, anti-rug parameters, or leader wallets.",
      completed: currentStep > 3,
    },
    {
      step: 4,
      title: "Live Execution Activation",
      desc: "Activate bot runtime processes under autonomous lease fencing.",
      completed: currentStep > 4,
    },
  ];

  return (
    <AppShell title="Tenant Onboarding">
      <div style={{ maxWidth: "800px", margin: "0 auto" }}>
        <div style={{ marginBottom: "2rem", textAlign: "center" }}>
          <h1 style={{ margin: 0 }}>Welcome to Sniper Suite Enterprise</h1>
          <p style={{ margin: "0.5rem 0 0", color: "var(--muted)", fontSize: "0.95rem" }}>
            Complete the 4-step onboarding checklist to configure your dedicated trading runtime.
          </p>
        </div>

        <div style={{ display: "flex", flexDirection: "column", gap: "1rem", marginBottom: "2rem" }}>
          {steps.map((s) => (
            <div
              key={s.step}
              className="card"
              style={{
                borderColor: s.step === currentStep ? "var(--accent)" : "var(--line)",
                background: s.step === currentStep ? "var(--panel-2)" : "var(--panel)",
              }}
            >
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                <div style={{ display: "flex", alignItems: "center", gap: "1rem" }}>
                  <div
                    style={{
                      width: "32px",
                      height: "32px",
                      borderRadius: "50%",
                      background: s.completed ? "var(--ok)" : s.step === currentStep ? "var(--accent)" : "rgba(255,255,255,0.1)",
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "center",
                      fontWeight: 700,
                      fontSize: "0.85rem",
                    }}
                  >
                    {s.completed ? "✓" : s.step}
                  </div>
                  <div>
                    <h3 style={{ margin: 0, fontSize: "1rem" }}>{s.title}</h3>
                    <p style={{ margin: "0.2rem 0 0", color: "var(--muted)", fontSize: "0.85rem" }}>{s.desc}</p>
                  </div>
                </div>

                {s.step === currentStep && (
                  <button
                    onClick={() => setCurrentStep(Math.min(4, currentStep + 1))}
                    className="btn btn-primary"
                    style={{ fontSize: "0.85rem" }}
                  >
                    {s.step === 4 ? "Complete Setup" : "Proceed →"}
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>

        <div className="card" style={{ textAlign: "center", padding: "1.5rem" }}>
          <h3 style={{ margin: 0 }}>Need quick access to the trading desk?</h3>
          <p style={{ color: "var(--muted)", fontSize: "0.85rem", margin: "0.5rem 0 1rem" }}>
            You can return to configuration settings or jump directly to active execution consoles.
          </p>
          <div style={{ display: "flex", justifyContent: "center", gap: "1rem" }}>
            <Link href="/strategies" className="btn btn-secondary">
              View Strategies
            </Link>
            <Link href="/trading" className="btn btn-primary">
              Open Trading Console
            </Link>
          </div>
        </div>
      </div>
    </AppShell>
  );
}
