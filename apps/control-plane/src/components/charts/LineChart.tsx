import { ChartPoint, isPlottable, linePoints, niceTicks, toPointsAttr } from "./scale";

interface LineChartProps {
  series: ChartPoint[];
  height?: number;
  width?: number;
  yLabel?: string;
  formatY?: (value: number) => string;
  ariaLabel?: string;
}

const PAD_LEFT = 56;
const PAD_RIGHT = 12;
const PAD_TOP = 12;
const PAD_BOTTOM = 28;

/**
 * Dependency-free SVG line chart. No randomness, no external fonts, and an
 * explicit "no data" state instead of an invented line.
 */
export function LineChart({
  series,
  height = 220,
  width = 640,
  yLabel,
  formatY = (v) => String(v),
  ariaLabel,
}: LineChartProps) {
  const values = series.map((p) => p.y).filter(isPlottable);
  if (values.length === 0) {
    return (
      <div role="img" aria-label={ariaLabel ?? "No data"} style={{ color: "var(--muted)", padding: "1rem" }}>
        No data yet.
      </div>
    );
  }

  const ticks = niceTicks(Math.min(...values), Math.max(...values), 5);
  const minY = ticks[0] ?? Math.min(...values);
  const maxY = ticks[ticks.length - 1] ?? Math.max(...values);
  const innerW = width - PAD_LEFT - PAD_RIGHT;
  const innerH = height - PAD_TOP - PAD_BOTTOM;
  const points = linePoints(values, innerW, innerH, minY, maxY).map((p) => ({
    x: p.x + PAD_LEFT,
    y: p.y + PAD_TOP,
  }));
  const first = series.find((p) => isPlottable(p.y));
  const last = [...series].reverse().find((p) => isPlottable(p.y));

  return (
    <svg
      role="img"
      aria-label={ariaLabel ?? (yLabel ? `${yLabel} over time` : "Line chart")}
      viewBox={`0 0 ${width} ${height}`}
      width="100%"
      style={{ maxWidth: width, display: "block" }}
    >
      <title>{yLabel ?? "Line chart"}</title>
      {ticks.map((tick) => {
        const y = PAD_TOP + innerH - ((tick - minY) / (maxY - minY || 1)) * innerH;
        return (
          <g key={`t-${tick}`}>
            <line x1={PAD_LEFT} x2={width - PAD_RIGHT} y1={y} y2={y} stroke="var(--line, #2a2f3a)" strokeWidth={1} />
            <text x={PAD_LEFT - 8} y={y + 4} textAnchor="end" fontSize={11} fill="var(--muted, #8b93a7)">
              {formatY(tick)}
            </text>
          </g>
        );
      })}
      <polyline
        points={toPointsAttr(points)}
        fill="none"
        stroke="var(--accent, #2f6fed)"
        strokeWidth={2}
        strokeLinejoin="round"
      />
      {first ? (
        <text x={PAD_LEFT} y={height - 8} fontSize={11} fill="var(--muted, #8b93a7)">
          {String(first.x)}
        </text>
      ) : null}
      {last ? (
        <text x={width - PAD_RIGHT} y={height - 8} textAnchor="end" fontSize={11} fill="var(--muted, #8b93a7)">
          {String(last.x)}
        </text>
      ) : null}
    </svg>
  );
}

export default LineChart;
