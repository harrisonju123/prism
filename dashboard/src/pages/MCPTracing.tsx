import { useState, useMemo } from "react";
import type { TimeRange } from "../hooks/useStats";
import { useMCPServers, useMCPWaste } from "../hooks/useMCP";
import { CardShell } from "../components/common/CardShell";
import { InfoTip } from "../components/common/InfoTip";
import { formatMs } from "../utils/format";
import type { MCPServer } from "../api/types";

interface Props {
  timeRange: TimeRange;
}

export function MCPTracing({ timeRange }: Props) {
  const [selectedServer, setSelectedServer] = useState<string | null>(null);

  return (
    <div className="flex flex-col gap-6 max-w-[1600px] mx-auto">
      <h1 className="text-lg font-semibold flex items-center gap-2">MCP Tracing <InfoTip text="Tracks Model Context Protocol tool calls made by AI agents — reliability, latency, and wasted data." /></h1>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        <div className="lg:col-span-2">
          <ServerList
            timeRange={timeRange}
            selectedServer={selectedServer}
            onSelectServer={setSelectedServer}
          />
        </div>
        <div>
          <WastePanel timeRange={timeRange} />
        </div>
      </div>

      {selectedServer && (
        <ServerDetail serverName={selectedServer} timeRange={timeRange} />
      )}
    </div>
  );
}

// -- Server List --------------------------------------------------------------

function ServerList({
  timeRange,
  selectedServer,
  onSelectServer,
}: {
  timeRange: TimeRange;
  selectedServer: string | null;
  onSelectServer: (name: string | null) => void;
}) {
  const { data, isLoading, error } = useMCPServers(timeRange);

  const sorted = useMemo(() => {
    if (!data?.stats) return [];
    return [...data.stats].sort((a, b) => b.call_count - a.call_count);
  }, [data?.stats]);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-medium text-gray-300">MCP Servers</h2>
        {data && (
          <span className="text-[10px] text-gray-500">
            {data.stats.length} server{data.stats.length !== 1 ? "s" : ""}
          </span>
        )}
      </div>
      <CardShell loading={isLoading} error={error ?? null} skeletonHeight="h-64">
        {sorted.length === 0 && !isLoading && (
          <p className="text-xs text-gray-500 py-4 text-center">
            No MCP servers detected yet
          </p>
        )}
        {sorted.length > 0 && (
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="text-gray-500 border-b border-gray-800">
                  <th className="text-left py-2 px-2 font-medium">Server</th>
                  <th className="text-right py-2 px-2 font-medium">Calls</th>
                  <th className="text-right py-2 px-2 font-medium">Failures</th>
                  <th className="text-right py-2 px-2 font-medium">Fail Rate</th>
                  <th className="text-right py-2 px-2 font-medium">Avg Latency</th>
                  <th className="text-right py-2 px-2 font-medium">P50</th>
                  <th className="text-right py-2 px-2 font-medium">P95</th>
                </tr>
              </thead>
              <tbody>
                {sorted.map((s) => (
                  <tr
                    key={s.server_name}
                    onClick={() =>
                      onSelectServer(
                        selectedServer === s.server_name ? null : s.server_name,
                      )
                    }
                    className={[
                      "border-b border-gray-800/50 transition-colors cursor-pointer",
                      selectedServer === s.server_name
                        ? "bg-violet-600/10"
                        : "hover:bg-gray-800/30",
                    ].join(" ")}
                  >
                    <td className="py-2 px-2 font-mono text-gray-200">{s.server_name}</td>
                    <td className="py-2 px-2 text-right font-mono text-gray-300">
                      {s.call_count.toLocaleString()}
                    </td>
                    <td className="py-2 px-2 text-right font-mono text-gray-300">
                      {s.failure_count.toLocaleString()}
                    </td>
                    <td className="py-2 px-2 text-right">
                      <FailRateBadge rate={s.failure_rate} />
                    </td>
                    <td className="py-2 px-2 text-right font-mono text-gray-300">
                      {s.avg_latency_ms != null ? formatMs(s.avg_latency_ms) : "---"}
                    </td>
                    <td className="py-2 px-2 text-right font-mono text-gray-300">
                      {s.p50_latency_ms != null ? formatMs(s.p50_latency_ms) : "---"}
                    </td>
                    <td className="py-2 px-2 text-right font-mono text-gray-300">
                      {s.p95_latency_ms != null ? formatMs(s.p95_latency_ms) : "---"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </CardShell>
    </div>
  );
}

// -- Server Detail (expanded row for selected server) -------------------------

function ServerDetail({
  serverName,
  timeRange,
}: {
  serverName: string;
  timeRange: TimeRange;
}) {
  const { data } = useMCPServers(timeRange);

  const server: MCPServer | undefined = useMemo(
    () => data?.stats.find((s) => s.server_name === serverName),
    [data?.stats, serverName],
  );

  if (!server) return null;

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <h2 className="text-sm font-medium text-gray-300">
          Server Detail:
        </h2>
        <span className="font-mono text-sm text-violet-300">{serverName}</span>
      </div>
      <CardShell>
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
          <StatCard label="Total Calls" value={server.call_count.toLocaleString()} />
          <StatCard label="Failures" value={server.failure_count.toLocaleString()} />
          <StatCard
            label="Failure Rate"
            value={`${(server.failure_rate * 100).toFixed(1)}%`}
            alert={server.failure_rate > 0.1}
          />
          <StatCard
            label="Avg Latency"
            value={server.avg_latency_ms != null ? formatMs(server.avg_latency_ms) : "---"}
          />
        </div>
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-4 mt-4 pt-4 border-t border-gray-800">
          <StatCard
            label="P50 Latency"
            value={server.p50_latency_ms != null ? formatMs(server.p50_latency_ms) : "---"}
          />
          <StatCard
            label="P95 Latency"
            value={server.p95_latency_ms != null ? formatMs(server.p95_latency_ms) : "---"}
          />
        </div>
      </CardShell>
    </div>
  );
}

// -- Waste Panel --------------------------------------------------------------

function WastePanel({ timeRange }: { timeRange: TimeRange }) {
  const { data, isLoading, error } = useMCPWaste(timeRange);

  return (
    <div className="flex flex-col gap-3">
      <h2 className="text-sm font-medium text-gray-300 flex items-center gap-1">Unused Data Waste <InfoTip text="MCP calls where the response was included in context but never referenced in the completion, inflating token costs." /></h2>
      <CardShell loading={isLoading} error={error ?? null} skeletonHeight="h-48">
        {data?.waste.length === 0 && (
          <p className="text-xs text-gray-500 py-4 text-center">
            No wasted MCP calls detected
          </p>
        )}
        {data && data.waste.length > 0 && (
          <div className="flex flex-col gap-2 max-h-[400px] overflow-y-auto">
            {data.waste.map((w) => (
              <div
                key={`${w.server_name}:${w.method}`}
                className="flex flex-col gap-1 px-3 py-2 bg-gray-800/30 rounded"
              >
                <div className="flex items-center justify-between">
                  <span className="text-xs font-mono text-gray-200">
                    {w.server_name}
                  </span>
                  <span className="text-[10px] text-yellow-400 bg-yellow-500/10 px-1.5 py-0.5 rounded">
                    {w.unused_call_count} unused
                  </span>
                </div>
                <div className="flex items-center justify-between">
                  <span className="text-[10px] text-gray-400">{w.method}</span>
                  <span className="text-[10px] text-gray-500 font-mono">
                    {w.total_wasted_tokens.toLocaleString()} tokens wasted
                  </span>
                </div>
              </div>
            ))}
          </div>
        )}
      </CardShell>
    </div>
  );
}

// -- Shared small components --------------------------------------------------

function FailRateBadge({ rate }: { rate: number }) {
  const pct = (rate * 100).toFixed(1);
  const color =
    rate > 0.1
      ? "bg-red-500/15 text-red-400"
      : rate > 0.01
        ? "bg-yellow-500/15 text-yellow-400"
        : "bg-green-500/15 text-green-400";

  return (
    <span className={`inline-block px-1.5 py-0.5 rounded text-[10px] font-medium font-mono ${color}`}>
      {pct}%
    </span>
  );
}

function StatCard({
  label,
  value,
  alert = false,
}: {
  label: string;
  value: string;
  alert?: boolean;
}) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-[10px] text-gray-500 uppercase tracking-wider">{label}</span>
      <span className={`text-sm font-mono ${alert ? "text-red-400" : "text-gray-100"}`}>
        {value}
      </span>
    </div>
  );
}
