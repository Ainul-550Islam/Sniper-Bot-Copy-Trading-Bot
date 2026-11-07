"use client";

import { BacktestRecord } from "@/lib/api/backtest-api";

/**
 * Plain-language status for a backtest. Every number on the page must sit
 * next to this banner, so a synthetic simulation can never pass as market
 * evidence.
 */
const BLOCK_TEXT: Record<string, string> = {
  no_recorded_dataset:
    "Waiting: this server has no recorded market dataset and synthetic runs are disabled. Nothing has been simulated.",
  recorded_dataset_unavailable:
    "Blocked: this run names a recorded dataset, but replay over recorded data is not available yet. Nothing has been simulated.",
};

export function BacktestTruthBanner({ backtest }: { backtest: BacktestRecord }) {
  const blocked = backtest.blocked_reason ? BLOCK_TEXT[backtest.blocked_reason] ?? backtest.blocked_reason : null;
  const isSynthetic = backtest.data_source === "synthetic";

  let text: string;
  let tone: string;
  if (blocked) {
    text = blocked;
    tone = "var(--warn, #b7791f)";
  } else if (isSynthetic && backtest.authoritative) {
    text =
      "Provenance conflict: this result is labeled synthetic but also authoritative. Do not treat it as a recorded-data result.";
    tone = "var(--bad)";
  } else if (isSynthetic) {
    text =
      "SYNTHETIC SIMULATION — not based on recorded market data and not authoritative. Use it only to compare settings, not as proof of performance.";
    tone = "var(--warn, #b7791f)";
  } else if (backtest.data_source === "recorded" && backtest.status === "completed" && backtest.authoritative) {
    text = "Recorded-data result.";
    tone = "var(--ok)";
  } else if (backtest.authoritative && backtest.data_source !== "recorded") {
    text = "Provenance conflict: the result is marked authoritative without a recorded data source. Do not rely on it as recorded-data evidence.";
    tone = "var(--bad)";
  } else if (backtest.data_source === "recorded" && !backtest.authoritative) {
    text = "Recorded market data is reported, but this result is not marked authoritative.";
    tone = "var(--warn, #b7791f)";
  } else {
    text = "Result pending.";
    tone = "var(--muted)";
  }

  return (
    <div
      role="note"
      data-testid="backtest-truth-banner"
      style={{
        border: `1px solid ${tone}`,
        borderRadius: "6px",
        padding: "0.75rem 1rem",
        marginBottom: "1rem",
        color: tone,
        fontSize: "0.9rem",
      }}
    >
      {text}
    </div>
  );
}

export default BacktestTruthBanner;
