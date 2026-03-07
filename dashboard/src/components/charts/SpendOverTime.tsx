import { useMemo } from "react";
import {
  AreaChart,
  Area,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import { CardShell } from "../common/CardShell";
import { useTimeseries } from "../../hooks/useStats";
import type { TimeRange } from "../../hooks/useStats";
import { formatTimestamp, formatUSD } from "../../utils/format";

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
    <div className="glass-card rounded px-3 py-2 text-xs border-l-2 border-l-violet-500" style={{ transform: 'none' }}>
      <p className="text-[var(--text-secondary)] mb-1">
        {new Date(point.payload.timestamp).toLocaleString()}
      </p>
      <p className="text-violet-300 font-mono">{formatUSD(point.value)}</p>
    </div>
  );
}

export function SpendOverTime({ timeRange }: Props) {
  const { data, isLoading, error } = useTimeseries("cost", timeRange);

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
      title="Spend over time"
      loading={isLoading}
      error={error}
      skeletonHeight="h-52"
      className="min-h-[16rem]"
    >
      <ResponsiveContainer width="100%" height={200}>
        <AreaChart data={chartData} margin={{ top: 4, right: 8, left: 0, bottom: 0 }}>
          <defs>
            <linearGradient id="spendGradient" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="#8b5cf6" stopOpacity={0.4} />
              <stop offset="60%" stopColor="#8b5cf6" stopOpacity={0.08} />
              <stop offset="100%" stopColor="#8b5cf6" stopOpacity={0} />
            </linearGradient>
            <filter id="glowViolet">
              <feGaussianBlur stdDeviation="3" result="blur" />
              <feMerge>
                <feMergeNode in="blur" />
                <feMergeNode in="SourceGraphic" />
              </feMerge>
            </filter>
          </defs>
          <CartesianGrid strokeDasharray="3 3" stroke="rgba(255,255,255,0.04)" />
          <XAxis
            dataKey="label"
            tick={{ fontSize: 10, fill: "#64748b" }}
            tickLine={false}
            axisLine={{ stroke: "rgba(255,255,255,0.06)" }}
            interval="preserveStartEnd"
          />
          <YAxis
            tickFormatter={formatUSD}
            tick={{ fontSize: 10, fill: "#64748b" }}
            tickLine={false}
            axisLine={false}
            width={56}
          />
          <Tooltip content={<CustomTooltip />} />
          <Area
            type="monotone"
            dataKey="value"
            stroke="#8b5cf6"
            strokeWidth={2}
            fill="url(#spendGradient)"
            dot={false}
            activeDot={{ r: 4, fill: "#8b5cf6", stroke: "#020409", strokeWidth: 2 }}
            animationDuration={1200}
            animationEasing="ease-out"
          />
        </AreaChart>
      </ResponsiveContainer>
    </CardShell>
  );
}
