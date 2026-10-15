import { isPlottable, linePoints, toPointsAttr } from "./scale";

interface SparklineProps {
  values: number[];
  width?: number;
  height?: number;
  ariaLabel?: string;
}

/** 120×32 trend line for summary cards. Fewer than two points draws nothing. */
export function Sparkline({ values, width = 120, height = 32, ariaLabel = "Trend" }: SparklineProps) {
  const clean = values.filter(isPlottable);
  if (clean.length < 2) {
    return (
      <svg role="img" aria-label={`${ariaLabel}: not enough data`} width={width} height={height} viewBox={`0 0 ${width} ${height}`}>
        <line x1={0} x2={width} y1={height / 2} y2={height / 2} stroke="var(--line, #2a2f3a)" strokeDasharray="3 3" />
      </svg>
    );
  }
  const min = Math.min(...clean);
  const max = Math.max(...clean);
  const points = linePoints(clean, width, height, min, max);
  const rising = (clean[clean.length - 1] ?? 0) >= (clean[0] ?? 0);
  return (
    <svg role="img" aria-label={ariaLabel} width={width} height={height} viewBox={`0 0 ${width} ${height}`}>
      <polyline
        points={toPointsAttr(points)}
        fill="none"
        stroke={rising ? "var(--ok, #22c55e)" : "var(--bad, #ef4444)"}
        strokeWidth={1.5}
        strokeLinejoin="round"
      />
    </svg>
  );
}

export default Sparkline;
