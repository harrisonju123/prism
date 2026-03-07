import NumberFlow from "@number-flow/react";
import { CardShell } from "../common/CardShell";
import { InfoTip } from "../common/InfoTip";
import { useWasteScore } from "../../hooks/useStats";
import type { TimeRange } from "../../hooks/useStats";

interface Props {
  timeRange: TimeRange;
}

function scoreColor(score: number): { ring: string; text: string; glow: string } {
  if (score < 0.2) return { ring: "#34d399", text: "text-emerald-400", glow: "rgba(52,211,153,0.3)" };
  if (score < 0.5) return { ring: "#d97706", text: "text-amber-400", glow: "rgba(217,119,6,0.3)" };
  return { ring: "#dc2626", text: "text-red-400", glow: "rgba(220,38,38,0.3)" };
}

function scoreLabel(score: number): string {
  if (score < 0.2) return "Low waste";
  if (score < 0.5) return "Moderate waste";
  return "High waste";
}

function confidenceBadge(
  confidence: number,
  source?: "fitness" | "heuristic" | null,
): { label: string; className: string } {
  if (source === "heuristic") {
    return { label: "est", className: "bg-white/[0.03] text-[var(--text-secondary)] border-white/[0.06]" };
  }
  if (confidence >= 0.8) return { label: "high", className: "bg-emerald-900/40 text-emerald-400 border-emerald-700/40" };
  if (confidence >= 0.5) return { label: "med", className: "bg-amber-900/40 text-amber-400 border-amber-700/40" };
  return { label: "low", className: "bg-white/[0.03] text-[var(--text-secondary)] border-white/[0.06]" };
}

interface RingGaugeProps {
  value: number;
  color: string;
  glow: string;
  size?: number;
}

function RingGauge({ value, color, glow, size = 120 }: RingGaugeProps) {
  const strokeWidth = 8;
  const radius = (size - strokeWidth) / 2;
  const circumference = 2 * Math.PI * radius;
  const offset = circumference * (1 - Math.min(value, 1));

  return (
    <svg width={size} height={size} className="block mx-auto">
      <defs>
        <filter id="ringGlow">
          <feGaussianBlur stdDeviation="4" result="blur" />
          <feMerge>
            <feMergeNode in="blur" />
            <feMergeNode in="SourceGraphic" />
          </feMerge>
        </filter>
      </defs>
      {/* Track */}
      <circle
        cx={size / 2}
        cy={size / 2}
        r={radius}
        fill="none"
        stroke="rgba(255,255,255,0.05)"
        strokeWidth={strokeWidth}
      />
      {/* Filled arc with glow */}
      <circle
        cx={size / 2}
        cy={size / 2}
        r={radius}
        fill="none"
        stroke={color}
        strokeWidth={strokeWidth}
        strokeLinecap="round"
        strokeDasharray={circumference}
        strokeDashoffset={offset}
        className="transition-all duration-700"
        transform={`rotate(-90 ${size / 2} ${size / 2})`}
        style={{ filter: `drop-shadow(0 0 8px ${glow})` }}
      />
    </svg>
  );
}

export function WasteScore({ timeRange }: Props) {
  const { data, isLoading, error } = useWasteScore(timeRange);

  const pct = data ? data.waste_score * 100 : 0;
  const colors = scoreColor(data?.waste_score ?? 0);

  return (
    <CardShell
      title={<span className="flex items-center gap-1">Waste score <InfoTip text="Estimated percentage of spend that could be saved by routing to cheaper models without meaningful quality loss." /></span>}
      loading={isLoading}
      error={error}
      skeletonHeight="h-32"
    >
      <div className="flex flex-col gap-4">
        {/* Ring gauge with centered label */}
        <div className="relative">
          <RingGauge value={data?.waste_score ?? 0} color={colors.ring} glow={colors.glow} />
          <div className="absolute inset-0 flex flex-col items-center justify-center">
            <NumberFlow
              value={Math.round(pct)}
              suffix="%"
              className={`text-3xl font-mono font-semibold ${colors.text}`}
              transformTiming={{ duration: 800, easing: "cubic-bezier(0.16, 1, 0.3, 1)" }}
            />
            <span className="text-[10px] text-[var(--text-muted)]">
              {scoreLabel(data?.waste_score ?? 0)}
            </span>
          </div>
        </div>

        {/* Potential savings */}
        <div className="text-xs text-[var(--text-secondary)] text-center">
          Potential savings:{" "}
          <span className="font-mono text-[var(--text-primary)]">
            ${(data?.total_potential_savings_usd ?? 0).toFixed(2)}
          </span>
        </div>

        {/* Breakdown */}
        {(data?.breakdown ?? []).length > 0 && (
          <div className="border-t border-white/[0.06] pt-3">
            <p className="text-[0.625rem] text-[var(--text-muted)] mb-2 uppercase tracking-[0.15em]">
              Top suggestions
            </p>
            <ul className="space-y-1.5">
              {data!.breakdown.slice(0, 4).map((item, i) => {
                const badge = confidenceBadge(item.confidence, item.suggestion_source);
                return (
                  <li key={i} className="flex items-center justify-between text-xs gap-2">
                    <span className="text-[var(--text-secondary)] truncate flex-1 min-w-0">
                      {item.current_model}
                      <span className="text-[var(--text-muted)] mx-1">→</span>
                      {item.suggested_model}
                    </span>
                    <span
                      className={`shrink-0 px-1.5 py-0.5 rounded text-[10px] border ${badge.className}`}
                    >
                      {badge.label}
                    </span>
                    <span className="font-mono text-emerald-400 shrink-0">
                      save ${item.savings_usd.toFixed(2)}
                    </span>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </div>
    </CardShell>
  );
}
