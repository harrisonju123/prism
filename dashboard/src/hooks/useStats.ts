import { useQuery } from "@tanstack/react-query";
import {
  getSummary,
  getTimeseries,
  getTopTraces,
  getWasteScore,
} from "../api/client";
import type { TimeseriesResponse } from "../api/types";

// Interval selection is driven by the time range so charts don't show
// thousands of 1h buckets for a 30-day window.
export function intervalForRange(range: TimeRange): TimeseriesResponse["interval"] {
  if (range === "30d") return "1d";
  if (range === "7d") return "6h";
  return "1h";
}

// The three time range options surfaced in the UI.
export type TimeRange = "24h" | "7d" | "30d";

// Derive ISO start/end strings from a relative range label.
// We pass explicit timestamps to the API so refetches always use the
// same window (the query key captures the rounded time).
export function rangeToParams(range: TimeRange): { start: string; end: string } {
  const end = new Date();
  const start = new Date(end);
  if (range === "24h") start.setHours(start.getHours() - 24);
  else if (range === "7d") start.setDate(start.getDate() - 7);
  else start.setDate(start.getDate() - 30);
  return { start: start.toISOString(), end: end.toISOString() };
}

// Shift a range backward by its own duration to get the prior period.
function previousRangeToParams(range: TimeRange): { start: string; end: string } {
  const now = new Date();
  const currentStart = new Date(now);
  if (range === "24h") currentStart.setHours(currentStart.getHours() - 24);
  else if (range === "7d") currentStart.setDate(currentStart.getDate() - 7);
  else currentStart.setDate(currentStart.getDate() - 30);

  // Previous period ends where current starts
  const prevEnd = new Date(currentStart);
  const prevStart = new Date(prevEnd);
  if (range === "24h") prevStart.setHours(prevStart.getHours() - 24);
  else if (range === "7d") prevStart.setDate(prevStart.getDate() - 7);
  else prevStart.setDate(prevStart.getDate() - 30);

  return { start: prevStart.toISOString(), end: prevEnd.toISOString() };
}

export function useSummary(range: TimeRange, groupBy = "model") {
  const { start, end } = rangeToParams(range);
  return useQuery({
    queryKey: ["summary", range, groupBy],
    queryFn: () => getSummary(start, end, groupBy),
    staleTime: 30_000,
  });
}

export function usePreviousSummary(range: TimeRange, groupBy = "model") {
  const { start, end } = previousRangeToParams(range);
  return useQuery({
    queryKey: ["summary-prev", range, groupBy],
    queryFn: () => getSummary(start, end, groupBy),
    staleTime: 30_000,
  });
}

export function useTimeseries(
  metric: TimeseriesResponse["metric"],
  range: TimeRange
) {
  const { start, end } = rangeToParams(range);
  const interval = intervalForRange(range);
  return useQuery({
    queryKey: ["timeseries", metric, range],
    queryFn: () => getTimeseries(metric, interval, start, end),
    staleTime: 30_000,
  });
}

export function useTopTraces(range: TimeRange) {
  const { start, end } = rangeToParams(range);
  return useQuery({
    queryKey: ["top-traces", range],
    queryFn: () => getTopTraces("cost", 10, start, end),
    staleTime: 30_000,
  });
}

export function useWasteScore(range: TimeRange) {
  const { start, end } = rangeToParams(range);
  return useQuery({
    queryKey: ["waste-score", range],
    queryFn: () => getWasteScore(start, end),
    staleTime: 30_000,
  });
}
