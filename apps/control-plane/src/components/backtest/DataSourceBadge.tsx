import type { BacktestRecord } from "@/lib/api/backtest-api";

export interface DataSourceBadgeProps {
  dataSource: BacktestRecord["data_source"];
  authoritative: BacktestRecord["authoritative"];
}

function sourceLabel(dataSource: string | null): string {
  if (dataSource === "synthetic") return "Simulated data";
  if (dataSource === "recorded") return "Recorded market data";
  return dataSource ? `Unrecognized data source (${dataSource})` : "Data source unavailable";
}

/** Single source of truth for the backtest data provenance disclosure. */
export default function DataSourceBadge({
  dataSource,
  authoritative,
}: DataSourceBadgeProps) {
  const source = sourceLabel(dataSource);
  const authority = authoritative ? "Authoritative" : "Not authoritative";
  return (
    <span
      className={`badge ${authoritative ? "badge-ok" : "badge-warn"}`}
      role="status"
      aria-label={`${source}; ${authority}`}
      title={`${source}; ${authority}`}
    >
      {source} · {authority}
    </span>
  );
}
