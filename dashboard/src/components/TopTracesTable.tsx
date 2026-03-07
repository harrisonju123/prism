import { useMemo, useState } from "react";
import { useTopTraces } from "../hooks/useStats";
import type { TimeRange } from "../hooks/useStats";
import { modelColor } from "./charts/modelColors";
import { formatMs, formatUSD } from "../utils/format";
import { CardShell } from "./common/CardShell";
import { InfoTip } from "./common/InfoTip";

interface Props {
  timeRange: TimeRange;
}

type SortKey = "total_cost_usd" | "total_tokens" | "event_count" | "total_latency_ms";
type SortDir = "asc" | "desc";

function truncateId(id: string): string {
  return id.slice(0, 8) + "...";
}

const RANK_GLOWS = [
  "drop-shadow(0 0 6px rgba(251,191,36,0.4))",   // gold
  "drop-shadow(0 0 6px rgba(203,213,225,0.3))",   // silver
  "drop-shadow(0 0 6px rgba(180,83,9,0.3))",      // bronze
];
const RANK_COLORS = ["text-amber-400", "text-gray-300", "text-amber-700"];

function SortHeader({
  label,
  sortKey,
  activeSortKey,
  sortDir,
  onSort,
}: {
  label: string;
  sortKey: SortKey;
  activeSortKey: SortKey;
  sortDir: SortDir;
  onSort: (key: SortKey) => void;
}) {
  const isActive = sortKey === activeSortKey;
  return (
    <th
      className="text-right py-1.5 pr-4 font-medium cursor-pointer select-none hover:text-[var(--text-primary)] transition-colors"
      onClick={() => onSort(sortKey)}
    >
      {label}{" "}
      {isActive && (
        <span className="text-violet-400">{sortDir === "desc" ? "▼" : "▲"}</span>
      )}
    </th>
  );
}

export function TopTracesTable({ timeRange }: Props) {
  const { data, isLoading, error } = useTopTraces(timeRange);
  const traces = data?.traces ?? [];

  const [sortKey, setSortKey] = useState<SortKey>("total_cost_usd");
  const [sortDir, setSortDir] = useState<SortDir>("desc");

  function handleSort(key: SortKey) {
    if (key === sortKey) {
      setSortDir((d) => (d === "desc" ? "asc" : "desc"));
    } else {
      setSortKey(key);
      setSortDir("desc");
    }
  }

  const sorted = useMemo(() => {
    const copy = [...traces];
    copy.sort((a, b) => {
      const av = a[sortKey] as number;
      const bv = b[sortKey] as number;
      return sortDir === "desc" ? bv - av : av - bv;
    });
    return copy;
  }, [traces, sortKey, sortDir]);

  return (
    <CardShell
      title={<span className="flex items-center gap-1">Top 10 traces by cost <InfoTip text="A trace groups all LLM calls in a single agent session. Shows the most expensive traces to spot runaway loops." /></span>}
      loading={isLoading}
      error={error}
      skeletonHeight="h-40"
    >
      <div className="overflow-x-auto">
        <table className="w-full text-xs">
          <thead>
            <tr className="border-b border-white/[0.06] text-[var(--text-muted)]">
              <th className="text-left py-1.5 pr-2 font-medium w-6">#</th>
              <th className="text-left py-1.5 pr-4 font-medium">Trace ID</th>
              <SortHeader label="Cost" sortKey="total_cost_usd" activeSortKey={sortKey} sortDir={sortDir} onSort={handleSort} />
              <SortHeader label="Tokens" sortKey="total_tokens" activeSortKey={sortKey} sortDir={sortDir} onSort={handleSort} />
              <SortHeader label="Calls" sortKey="event_count" activeSortKey={sortKey} sortDir={sortDir} onSort={handleSort} />
              <SortHeader label="Latency" sortKey="total_latency_ms" activeSortKey={sortKey} sortDir={sortDir} onSort={handleSort} />
              <th className="text-left py-1.5 pr-4 font-medium">Models</th>
              <th className="text-left py-1.5 font-medium">Framework</th>
            </tr>
          </thead>
          <tbody>
            {sorted.length === 0 ? (
              <tr>
                <td colSpan={8} className="py-12">
                  <div className="flex flex-col items-center gap-3 text-[var(--text-muted)]">
                    <svg
                      className="w-8 h-8 text-[var(--text-muted)]"
                      fill="none"
                      viewBox="0 0 24 24"
                      stroke="currentColor"
                      strokeWidth={1.5}
                    >
                      <path
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        d="M9 12h3.75M9 15h3.75M9 18h3.75m3 .75H18a2.25 2.25 0 002.25-2.25V6.108c0-1.135-.845-2.098-1.976-2.192a48.424 48.424 0 00-1.123-.08m-5.801 0c-.065.21-.1.433-.1.664 0 .414.336.75.75.75h4.5a.75.75 0 00.75-.75 2.25 2.25 0 00-.1-.664m-5.8 0A2.251 2.251 0 0113.5 2.25H15c1.012 0 1.867.668 2.15 1.586m-5.8 0c-.376.023-.75.05-1.124.08C9.095 4.01 8.25 4.973 8.25 6.108V8.25m0 0H4.875c-.621 0-1.125.504-1.125 1.125v11.25c0 .621.504 1.125 1.125 1.125h9.75c.621 0 1.125-.504 1.125-1.125V9.375c0-.621-.504-1.125-1.125-1.125H8.25z"
                      />
                    </svg>
                    <p className="text-sm">No traces recorded yet</p>
                    <p className="text-xs text-[var(--text-muted)]">
                      Route LLM calls through the proxy to start tracking
                    </p>
                  </div>
                </td>
              </tr>
            ) : (
              sorted.map((trace, i) => (
                <tr
                  key={trace.trace_id}
                  className="border-b border-white/[0.03] hover:bg-violet-500/[0.04] group transition-colors animate-fade-in"
                  style={{ animationDelay: `${i * 40}ms` }}
                >
                  <td className="py-1.5 pr-2 font-mono font-semibold relative">
                    <span
                      className={i < 3 ? RANK_COLORS[i] : "text-[var(--text-muted)]"}
                      style={i < 3 ? { filter: RANK_GLOWS[i] } : undefined}
                    >
                      {i + 1}
                    </span>
                  </td>
                  <td className="py-1.5 pr-4 font-mono text-[var(--text-primary)]">
                    <span className="relative">
                      {/* Violet left border on hover */}
                      <span className="absolute -left-3 top-0 bottom-0 w-0.5 bg-violet-500 scale-y-0 group-hover:scale-y-100 transition-transform origin-top rounded-full" />
                      {truncateId(trace.trace_id)}
                    </span>
                  </td>
                  <td className="py-1.5 pr-4 font-mono text-right text-violet-300">
                    {formatUSD(trace.total_cost_usd)}
                  </td>
                  <td className="py-1.5 pr-4 font-mono text-right text-[var(--text-primary)]">
                    {trace.total_tokens.toLocaleString()}
                  </td>
                  <td className="py-1.5 pr-4 font-mono text-right text-[var(--text-primary)]">
                    {trace.event_count}
                  </td>
                  <td className="py-1.5 pr-4 font-mono text-right text-[var(--text-secondary)]">
                    {formatMs(trace.total_latency_ms)}
                  </td>
                  <td className="py-1.5 pr-4">
                    <div className="flex gap-1 flex-wrap">
                      {trace.models_used.map((m) => (
                        <span
                          key={m}
                          className="px-1.5 py-0.5 rounded text-[10px] font-mono"
                          style={{
                            backgroundColor: modelColor(m) + "18",
                            color: modelColor(m),
                            border: `1px solid ${modelColor(m)}33`,
                          }}
                        >
                          {m.length > 18 ? m.slice(0, 16) + "..." : m}
                        </span>
                      ))}
                    </div>
                  </td>
                  <td className="py-1.5 text-[var(--text-muted)]">
                    {trace.agent_framework ?? "–"}
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </CardShell>
  );
}
