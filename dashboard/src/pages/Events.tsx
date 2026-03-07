import { useState, useMemo, useCallback, useEffect } from "react";
import type { TimeRange } from "../hooks/useStats";
import { useEvents, type EventFilters } from "../hooks/useEvents";
import { CardShell } from "../components/common/CardShell";
import { InfoTip } from "../components/common/InfoTip";
import { EventDrawer } from "../components/EventDrawer";
import { formatUSD, formatMs } from "../utils/format";

interface Props {
  timeRange: TimeRange;
}

const PAGE_SIZE = 50;

const STATUS_OPTIONS = ["all", "success", "failure"] as const;
const TASK_TYPES = [
  "all",
  "code_generation",
  "code_review",
  "classification",
  "summarization",
  "extraction",
  "reasoning",
  "conversation",
  "tool_selection",
  "unknown",
] as const;

type SortKey = "created_at" | "estimated_cost" | "latency_ms" | "total_tokens";
type SortDir = "asc" | "desc";

export function Events({ timeRange }: Props) {
  const [statusFilter, setStatusFilter] = useState<string>("all");
  const [taskTypeFilter, setTaskTypeFilter] = useState<string>("all");
  const [modelFilter, setModelFilter] = useState("");
  const [page, setPage] = useState(0);
  const [sortKey, setSortKey] = useState<SortKey>("created_at");
  const [sortDir, setSortDir] = useState<SortDir>("desc");
  const [selectedEventId, setSelectedEventId] = useState<string | null>(null);

  const handleCloseDrawer = useCallback(() => setSelectedEventId(null), []);

  // Reset pagination when time range changes
  useEffect(() => { setPage(0); }, [timeRange]);

  const filters: EventFilters = useMemo(() => {
    const f: EventFilters = { limit: PAGE_SIZE, offset: page * PAGE_SIZE };
    if (statusFilter !== "all") f.status = statusFilter as "success" | "failure";
    if (taskTypeFilter !== "all") f.task_type = taskTypeFilter;
    if (modelFilter) f.model = modelFilter;
    return f;
  }, [statusFilter, taskTypeFilter, modelFilter, page]);

  const { data, isLoading, error } = useEvents(timeRange, filters);

  // Client-side sort within the fetched page
  const sortedEvents = useMemo(() => {
    if (!data?.events) return [];
    return [...data.events].sort((a, b) => {
      const av = a[sortKey];
      const bv = b[sortKey];
      if (typeof av === "string" && typeof bv === "string") {
        return sortDir === "asc" ? av.localeCompare(bv) : bv.localeCompare(av);
      }
      return sortDir === "asc" ? (av as number) - (bv as number) : (bv as number) - (av as number);
    });
  }, [data?.events, sortKey, sortDir]);

  function toggleSort(key: SortKey) {
    if (sortKey === key) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDir("desc");
    }
  }

  function sortIcon(col: SortKey) {
    if (sortKey !== col) return null;
    return <span className="ml-1 text-violet-400">{sortDir === "asc" ? "\u2191" : "\u2193"}</span>;
  }

  return (
    <div className="flex flex-col gap-4 max-w-[1600px] mx-auto">
      <h1 className="text-lg font-semibold flex items-center gap-2">Events Explorer <InfoTip text="Every LLM API call captured by the proxy or callback, with classification and cost data. Click any row for full details." /></h1>

      {/* Filters */}
      <div className="flex flex-wrap gap-3 items-center">
        <select
          value={statusFilter}
          onChange={(e) => { setStatusFilter(e.target.value); setPage(0); }}
          className="bg-gray-800 border border-gray-700 rounded px-2 py-1.5 text-xs text-gray-200 focus:outline-none focus:ring-1 focus:ring-violet-500"
        >
          {STATUS_OPTIONS.map((s) => (
            <option key={s} value={s}>{s === "all" ? "All statuses" : s}</option>
          ))}
        </select>

        <select
          value={taskTypeFilter}
          onChange={(e) => { setTaskTypeFilter(e.target.value); setPage(0); }}
          className="bg-gray-800 border border-gray-700 rounded px-2 py-1.5 text-xs text-gray-200 focus:outline-none focus:ring-1 focus:ring-violet-500"
        >
          {TASK_TYPES.map((t) => (
            <option key={t} value={t}>{t === "all" ? "All task types" : t}</option>
          ))}
        </select>

        <input
          type="text"
          placeholder="Filter by model..."
          value={modelFilter}
          onChange={(e) => { setModelFilter(e.target.value); setPage(0); }}
          className="bg-gray-800 border border-gray-700 rounded px-2 py-1.5 text-xs text-gray-200 placeholder:text-gray-500 focus:outline-none focus:ring-1 focus:ring-violet-500 w-48"
        />

        {data && (
          <span className="text-xs text-gray-500 ml-auto">
            {data.total_count.toLocaleString()} events
          </span>
        )}
      </div>

      {/* Table */}
      <CardShell loading={isLoading} error={error ?? null} skeletonHeight="h-96">
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="text-gray-500 border-b border-gray-800">
                <th className="text-left py-2 px-2 font-medium cursor-pointer select-none" onClick={() => toggleSort("created_at")}>
                  Time{sortIcon("created_at")}
                </th>
                <th className="text-left py-2 px-2 font-medium">Status</th>
                <th className="text-left py-2 px-2 font-medium">Model</th>
                <th className="text-left py-2 px-2 font-medium">Task</th>
                <th className="text-right py-2 px-2 font-medium cursor-pointer select-none" onClick={() => toggleSort("total_tokens")}>
                  Tokens{sortIcon("total_tokens")}
                </th>
                <th className="text-right py-2 px-2 font-medium cursor-pointer select-none" onClick={() => toggleSort("estimated_cost")}>
                  Cost{sortIcon("estimated_cost")}
                </th>
                <th className="text-right py-2 px-2 font-medium cursor-pointer select-none" onClick={() => toggleSort("latency_ms")}>
                  Latency{sortIcon("latency_ms")}
                </th>
                <th className="text-left py-2 px-2 font-medium">Trace</th>
              </tr>
            </thead>
            <tbody>
              {sortedEvents.map((ev) => (
                <tr
                  key={ev.id}
                  onClick={() => setSelectedEventId(ev.id)}
                  className="border-b border-gray-800/50 hover:bg-gray-800/30 transition-colors cursor-pointer"
                >
                  <td className="py-2 px-2 text-gray-400 whitespace-nowrap">
                    {new Date(ev.created_at).toLocaleString("en-US", {
                      month: "short", day: "numeric", hour: "numeric", minute: "2-digit",
                    })}
                  </td>
                  <td className="py-2 px-2">
                    <span className={`inline-block px-1.5 py-0.5 rounded text-[10px] font-medium ${
                      ev.status === "success"
                        ? "bg-green-500/15 text-green-400"
                        : "bg-red-500/15 text-red-400"
                    }`}>
                      {ev.status}
                    </span>
                  </td>
                  <td className="py-2 px-2 font-mono text-gray-200 whitespace-nowrap">{ev.model}</td>
                  <td className="py-2 px-2 text-gray-400">
                    {ev.task_type ?? "---"}
                    {ev.task_type_confidence != null && (
                      <span className="text-gray-600 ml-1">({(ev.task_type_confidence * 100).toFixed(0)}%)</span>
                    )}
                  </td>
                  <td className="py-2 px-2 text-right font-mono text-gray-300">{ev.total_tokens.toLocaleString()}</td>
                  <td className="py-2 px-2 text-right font-mono text-gray-300">{formatUSD(ev.estimated_cost)}</td>
                  <td className="py-2 px-2 text-right font-mono text-gray-300">{formatMs(ev.latency_ms)}</td>
                  <td className="py-2 px-2 text-gray-500 font-mono truncate max-w-[120px]" title={ev.trace_id}>
                    {ev.trace_id.slice(0, 8)}
                  </td>
                </tr>
              ))}
              {sortedEvents.length === 0 && !isLoading && (
                <tr>
                  <td colSpan={8} className="py-8 text-center text-gray-500">No events found</td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </CardShell>

      {/* Pagination */}
      {data && data.total_count > PAGE_SIZE && (
        <div className="flex items-center justify-between">
          <button
            type="button"
            disabled={page === 0}
            onClick={() => setPage((p) => p - 1)}
            className="px-3 py-1 text-xs bg-gray-800 border border-gray-700 rounded disabled:opacity-40 hover:bg-gray-700 transition-colors"
          >
            Previous
          </button>
          <span className="text-xs text-gray-500">
            Page {page + 1} of {Math.ceil(data.total_count / PAGE_SIZE)}
          </span>
          <button
            type="button"
            disabled={!data.has_more}
            onClick={() => setPage((p) => p + 1)}
            className="px-3 py-1 text-xs bg-gray-800 border border-gray-700 rounded disabled:opacity-40 hover:bg-gray-700 transition-colors"
          >
            Next
          </button>
        </div>
      )}

      <EventDrawer eventId={selectedEventId} onClose={handleCloseDrawer} />
    </div>
  );
}
