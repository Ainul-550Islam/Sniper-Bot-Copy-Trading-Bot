import { barHeight, ChartBar, isPlottable, niceTicks } from "./scale";

interface BarChartProps {
  bars: ChartBar[];
  height?: number;
  width?: number;
  formatValue?: (value: number) => string;
  ariaLabel?: string;
}

const PAD_BOTTOM = 40;
const PAD_TOP = 12;

/** Dependency-free SVG bar chart (volume by module / day). */
export function BarChart({
  bars,
  height = 220,
  width = 640,
  formatValue = (v) => String(v),
  ariaLabel,
}: BarChartProps) {
  const valid = bars.filter((b) => isPlottable(b.value));
  if (valid.length === 0) {
    return (
      <div role="img" aria-label={ariaLabel ?? "No data"} style={{ color: "var(--muted)", padding: "1rem" }}>
        No data yet.
      </div>
    );
  }

  const max = Math.max(...valid.map((b) => b.value), 0);
  const ticks = niceTicks(0, max, 4);
  const top = ticks[ticks.length - 1] ?? max;
  const innerH = height - PAD_TOP - PAD_BOTTOM;
  const slot = width / valid.length;
  const barW = Math.max(4, slot * 0.6);

  return (
    <svg
      role="img"
      aria-label={ariaLabel ?? "Bar chart"}
      viewBox={`0 0 ${width} ${height}`}
      width="100%"
      style={{ maxWidth: width, display: "block" }}
    >
      <title>{ariaLabel ?? "Bar chart"}</title>
      {valid.map((bar, i) => {
        const h = barHeight(bar.value, top, innerH);
        const x = i * slot + (slot - barW) / 2;
        const y = PAD_TOP + innerH - h;
        return (
          <g key={`${bar.label}-${i}`}>
            <rect x={x} y={y} width={barW} height={h} fill="var(--accent, #2f6fed)">
              <title>{`${bar.label}: ${formatValue(bar.value)}`}</title>
            </rect>
            <text x={x + barW / 2} y={height - 22} textAnchor="middle" fontSize={11} fill="var(--muted, #8b93a7)">
              {bar.label}
            </text>
            <text x={x + barW / 2} y={height - 8} textAnchor="middle" fontSize={10} fill="var(--muted, #8b93a7)">
              {formatValue(bar.value)}
            </text>
          </g>
        );
      })}
    </svg>
  );
}

export default BarChart;
