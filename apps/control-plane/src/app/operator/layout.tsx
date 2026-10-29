"use client";

import React from "react";
import { AppShell } from "@/components/AppShell";
import { OperatorGuard } from "@/components/operator/OperatorGuard";

export default function OperatorLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <AppShell>
      <OperatorGuard>{children}</OperatorGuard>
    </AppShell>
  );
}
