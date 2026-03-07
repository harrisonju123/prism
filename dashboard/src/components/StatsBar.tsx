import { motion } from "framer-motion";
import NumberFlow from "@number-flow/react";
import { useSummary, usePreviousSummary } from "../hooks/useStats";
import type { TimeRange } from "../hooks/useStats";
import { formatUSD } from "../utils/format";
import { InfoTip } from "./common/InfoTip";

interface Props {
  timeRange: TimeRange;
}

type Sentiment = "positive" | "negative" | "neutral";

interface StatCardProps {
  label: string;
  value: string;
  rawNumber?: number;
  loading: boolean;
  delta?: number | null;
  /** Whether "up" is good or bad for this metric */
  upIsGood?: boolean;
  /** Conditional threshold coloring */
  valueClassName?: string;
  tip?: string;
  index: number;
}

function formatDelta(delta: number): string {
  const abs = Math.abs(delta);
  if (abs >= 100) return `${abs.toFixed(0)}%`;
  return `${abs.toFixed(1)}%`;
}

function deltaSentiment(delta: number, upIsGood: boolean): Sentiment {
  if (Math.abs(delta) < 0.5) return "neutral";
  const isUp = delta > 0;
  return (isUp === upIsGood) ? "positive" : "negative";
}

const SENTIMENT_COLORS: Record<Sentiment, string> = {
  positive: "text-emerald-400",
  negative: "text-red-400",
  neutral: "text-[var(--text-muted)]",
};

function StatCard({ label, value, rawNumber, loading, delta, upIsGood = true, valueClassName, tip, index }: StatCardProps) {
  const sentiment = delta != null ? deltaSentiment(delta, upIsGood) : "neutral";
  const arrow = delta != null && Math.abs(delta) >= 0.5
    ? (delta > 0 ? "▲" : "▼")
    : null;

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{
        duration: 0.5,
        delay: index * 0.06,
        ease: [0.16, 1, 0.3, 1],
      }}
      className="glass-card rounded-lg px-4 py-3 flex flex-col gap-1"
      style={{ borderTop: "1px solid rgba(139, 92, 246, 0.2)" }}
    >
      {loading ? (
        <div className="h-10 w-24 rounded bg-white/[0.03] relative overflow-hidden">
          <div className="animate-scan-line w-1/3 h-full bg-gradient-to-r from-transparent via-violet-500/10 to-transparent" />
        </div>
      ) : (
        <div className="flex items-baseline gap-2">
          {rawNumber != null ? (
            <NumberFlow
              value={rawNumber}
              format={label === "Total cost" ? { style: "currency", currency: "USD", minimumFractionDigits: 2 } : undefined}
              className={`text-3xl font-mono font-semibold tracking-[-0.02em] ${valueClassName ?? "text-[var(--text-hero)]"}`}
              transformTiming={{ duration: 800, easing: "cubic-bezier(0.16, 1, 0.3, 1)" }}
            />
          ) : (
            <span className={`text-3xl font-mono font-semibold tracking-[-0.02em] tabular-nums ${valueClassName ?? "text-[var(--text-hero)]"}`}>
              {value}
            </span>
          )}
          {arrow && delta != null && (
            <motion.span
              initial={{ opacity: 0, x: -8 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ duration: 0.3, delay: index * 0.06 + 0.3 }}
              className={`text-xs font-mono ${SENTIMENT_COLORS[sentiment]}`}
            >
              {arrow} {formatDelta(delta)}
            </motion.span>
          )}
        </div>
      )}
      <span className="text-[0.625rem] uppercase tracking-[0.15em] text-[var(--text-secondary)] flex items-center gap-1">{label} <InfoTip text={tip} /></span>
    </motion.div>
  );
}

function valueColor(metric: string, rawValue: number): string | undefined {
  if (metric === "failure") {
    if (rawValue > 0.10) return "text-red-400";
    if (rawValue > 0.05) return "text-amber-400";
  }
  if (metric === "latency") {
    if (rawValue > 5000) return "text-red-400";
    if (rawValue > 2000) return "text-amber-400";
  }
  return undefined;
}

export function StatsBar({ timeRange }: Props) {
  const { data, isLoading } = useSummary(timeRange);
  const { data: prevData } = usePreviousSummary(timeRange);

  const avgLatency = (() => {
    if (!data?.groups.length) return 0;
    const totalRequests = data.groups.reduce((s, g) => s + g.request_count, 0);
    if (totalRequests === 0) return 0;
    const weightedSum = data.groups.reduce(
      (s, g) => s + g.avg_latency_ms * g.request_count,
      0
    );
    return weightedSum / totalRequests;
  })();

  const prevAvgLatency = (() => {
    if (!prevData?.groups.length) return 0;
    const totalRequests = prevData.groups.reduce((s, g) => s + g.request_count, 0);
    if (totalRequests === 0) return 0;
    const weightedSum = prevData.groups.reduce(
      (s, g) => s + g.avg_latency_ms * g.request_count,
      0
    );
    return weightedSum / totalRequests;
  })();

  function pctChange(current: number, previous: number): number | null {
    if (!prevData || previous === 0) return null;
    return ((current - previous) / previous) * 100;
  }

  const cards = [
    {
      label: "Total requests",
      value: isLoading ? "–" : (data?.total_requests ?? 0).toLocaleString(),
      rawNumber: isLoading ? undefined : (data?.total_requests ?? 0),
      delta: pctChange(data?.total_requests ?? 0, prevData?.total_requests ?? 0),
      upIsGood: true,
    },
    {
      label: "Total cost",
      value: isLoading ? "–" : formatUSD(data?.total_cost_usd ?? 0),
      rawNumber: isLoading ? undefined : (data?.total_cost_usd ?? 0),
      delta: pctChange(data?.total_cost_usd ?? 0, prevData?.total_cost_usd ?? 0),
      upIsGood: false,
    },
    {
      label: "Avg latency",
      value: isLoading ? "–" : `${avgLatency.toFixed(0)} ms`,
      delta: pctChange(avgLatency, prevAvgLatency),
      upIsGood: false,
      valueClassName: valueColor("latency", avgLatency),
      tip: "Request-weighted mean latency across all models. High values may indicate throttling or oversized prompts.",
    },
    {
      label: "Failure rate",
      value: isLoading
        ? "–"
        : `${((data?.failure_rate ?? 0) * 100).toFixed(1)}%`,
      delta: pctChange(data?.failure_rate ?? 0, prevData?.failure_rate ?? 0),
      upIsGood: false,
      valueClassName: valueColor("failure", data?.failure_rate ?? 0),
      tip: "Percentage of LLM calls that returned an error or timed out.",
    },
  ];

  return (
    <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
      {cards.map((c, i) => (
        <StatCard
          key={c.label}
          label={c.label}
          value={c.value}
          rawNumber={c.rawNumber}
          loading={isLoading}
          delta={c.delta}
          upIsGood={c.upIsGood}
          valueClassName={c.valueClassName}
          tip={c.tip}
          index={i}
        />
      ))}
    </div>
  );
}
