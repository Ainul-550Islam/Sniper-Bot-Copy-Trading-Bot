// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { RiskLimitPanel } from "../components/risk/RiskLimitPanel";

afterEach(() => cleanup());

describe("risk limit utilization display", () => {
  it("does not convert a missing or invalid utilization percentage into zero", () => {
    render(<RiskLimitPanel referenceAsset="USD" rules={[
      { id: "rule-1", name: "Daily loss", scope: "organization", limit_ref: 1000, current_utilization_ref: 250, utilization_pct: null, status: "normal" },
      { id: "rule-2", name: "Drawdown", scope: "organization", limit_ref: 500, current_utilization_ref: 50, utilization_pct: Number.NaN, status: "normal" },
    ]} />);
    expect(screen.getAllByText("250 (percentage unavailable)")).toHaveLength(1);
    expect(screen.getAllByText("50 (percentage unavailable)")).toHaveLength(1);
    expect(screen.queryByText("250 (0.0%)")).toBeNull();
    expect(screen.queryByText("50 (0.0%)")).toBeNull();
  });
});
