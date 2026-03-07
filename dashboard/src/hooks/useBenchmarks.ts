import { useQuery } from "@tanstack/react-query";
import {
  getFitnessMatrix,
  getBenchmarkResults,
  getBenchmarkConfig,
  getBenchmarkDrift,
} from "../api/client";
import { rangeToParams, type TimeRange } from "./useStats";

export function useFitnessMatrix(range: TimeRange) {
  const { start, end } = rangeToParams(range);
  return useQuery({
    queryKey: ["fitness-matrix", range],
    queryFn: () => getFitnessMatrix(start, end),
    staleTime: 30_000,
  });
}

export function useBenchmarkResults(
  range: TimeRange,
  params: Record<string, string> = {}
) {
  const { start, end } = rangeToParams(range);
  return useQuery({
    queryKey: ["benchmark-results", range, params],
    queryFn: () =>
      getBenchmarkResults({ start, end, ...params }),
    staleTime: 30_000,
  });
}

export function useBenchmarkConfig() {
  return useQuery({
    queryKey: ["benchmark-config"],
    queryFn: getBenchmarkConfig,
    staleTime: 60_000,
  });
}

export function useBenchmarkDrift(range: TimeRange) {
  const { start, end } = rangeToParams(range);
  return useQuery({
    queryKey: ["benchmark-drift", range],
    queryFn: () => getBenchmarkDrift(start, end),
    staleTime: 30_000,
  });
}
