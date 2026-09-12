import { memo } from "react";
import { sampleSegments } from "@/lib/live";

/**
 * Mini-sparkline fuer die live-provider-tabelle. Reine svg-polylinien statt
 * chart-library, damit der 1s-render-takt guenstig bleibt.
 *
 * Auto-skala (scaleMax weggelassen/null): maximum der nicht-null-samples;
 * ohne positiven wert ist die sparkline eine flache grundlinie.
 */
export interface SparklineProps {
  samples: (number | null)[];
  color: string;
  width?: number;
  height?: number;
  /** feste max-skala fuer die y-achse; null = auto-skala aus den samples */
  scaleMax?: number | null;
  label: string;
}

function SparklineBase({
  samples,
  color,
  width = 96,
  height = 28,
  scaleMax = null,
  label,
}: SparklineProps) {
  const values = samples.filter((v): v is number => v !== null);
  const autoMax = values.reduce((m, v) => (v > m ? v : m), 0);
  const segments = sampleSegments(samples, width, height, scaleMax ?? autoMax);
  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      width={width}
      height={height}
      role="img"
      aria-label={label}
    >
      <line
        x1={0}
        y1={height}
        x2={width}
        y2={height}
        strokeWidth={1}
        style={{ stroke: "var(--border)" }}
      />
      {segments.map((points, i) => (
        <polyline
          key={i}
          points={points}
          fill="none"
          stroke={color}
          strokeWidth={1.5}
          strokeLinejoin="round"
          strokeLinecap="round"
        />
      ))}
    </svg>
  );
}

export const Sparkline = memo(SparklineBase);
