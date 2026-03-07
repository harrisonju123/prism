import { useQuery } from "@tanstack/react-query";
import { getMCPServers, getMCPGraph, getMCPWaste } from "../api/client";
import { rangeToParams, type TimeRange } from "./useStats";

/** Per-server call stats (count, latency, failure rate). */
export function useMCPServers(range: TimeRange) {
  const { start, end } = rangeToParams(range);
  return useQuery({
    queryKey: ["mcp-servers", range],
    queryFn: () => getMCPServers(start, end),
    staleTime: 30_000,
  });
}

/** Execution DAG for a single trace. Only fires when traceId is non-null. */
export function useMCPGraph(traceId: string | null) {
  return useQuery({
    queryKey: ["mcp-graph", traceId],
    queryFn: () => getMCPGraph(traceId!),
    enabled: traceId != null,
    staleTime: 60_000,
  });
}

/** Unused MCP data waste analysis. */
export function useMCPWaste(range: TimeRange) {
  const { start, end } = rangeToParams(range);
  return useQuery({
    queryKey: ["mcp-waste", range],
    queryFn: () => getMCPWaste(start, end),
    staleTime: 30_000,
  });
}
