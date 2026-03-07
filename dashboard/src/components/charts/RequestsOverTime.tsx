import { useMemo } from "react";
import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import { CardShell } from "../common/CardShell";
import { useTimeseries } from "../../hooks/useStats";
import type { TimeRange } from "../../hooks/useStats";
import { formatTimestamp } from "../../utils/format";

interface Props {
  timeRange: TimeRange;
}

interface TooltipPayload {
  value: number;
  payload: { timestamp: string };
}

function CustomTooltip({
  active,
  payload,
}: {
  active?: boolean;
  payload?: TooltipPayload[];
}) {
  if (!active || !payload?.length) return null;
  const point = payload[0];
  return (
    <div className="glass-card rounded px-3 py-2 text-xs border-l-2 border-l-cyan-500" style={{ transform: 'none' }}>
      <p className="text-[var(--text-secondary)] mb-1">
        {new Date(point.payload.timestamp).toLocaleString()}
      </p>
      <p className="text-cyan-300 font-mono">
        {point.value.toLocaleString()} requests
      </p>
    </div>
  );
}

export function RequestsOverTime({ timeRange }: Props) {
  const { data, isLoading, error } = useTimeseries("requests", timeRange);

  const chartData = useMemo(
    () =>
      data?.data.map((pt) => ({
        timestamp: pt.timestamp,
        label: formatTimestamp(pt.timestamp, timeRange),
        value: pt.value,
      })) ?? [],
    [data, timeRange]
  );

  return (
    <CardShell
      title="Requests over time"
      loading={isLoading}
      error={error}
      skeletonHeight="h-52"
      className="min-h-[16rem]"
    >
      <ResponsiveContainer width="100%" height={200}>
        <BarChart data={chartData} margin={{ top: 4, right: 8, left: 0, bottom: 0 }}>
          <defs>
            <linearGradient id="barGradientCyan" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="#22d3ee" stopOpacity={0.9} />
              <stop offset="100%" stopColor="#0e7490" stopOpacity={0.5} />
            </linearGradient>
          </defs>
          <CartesianGrid strokeDasharray="3 3" stroke="rgba(255,255,255,0.04)" vertical={false} />
          <XAxis
            dataKey="label"
            tick={{ fontSize: 10, fill: "#64748b" }}
            tickLine={false}
            axisLine={{ stroke: "rgba(255,255,255,0.06)" }}
            interval="preserveStartEnd"
          />
          <YAxis
            tick={{ fontSize: 10, fill: "#64748b" }}
            tickLine={false}
            axisLine={false}
            width={40}
          />
          <Tooltip content={<CustomTooltip />} cursor={{ fill: "rgba(255,255,255,0.03)" }} />
          <Bar
            dataKey="value"
            fill="url(#barGradientCyan)"
            radius={[2, 2, 0, 0]}
            maxBarSize={24}
            animationDuration={1200}
            animationEasing="ease-out"
          />
        </BarChart>
      </ResponsiveContainer>
    </CardShell>
  );
}
