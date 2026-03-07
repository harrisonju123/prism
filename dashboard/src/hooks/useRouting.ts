import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
  getRoutingPolicy,
  getRoutingDecisions,
  postDryRun,
  toggleRouting,
} from "../api/client";

export function useRoutingPolicy() {
  return useQuery({
    queryKey: ["routing-policy"],
    queryFn: getRoutingPolicy,
    staleTime: 30_000,
  });
}

export function useRoutingDecisions(limit = 50, offset = 0) {
  return useQuery({
    queryKey: ["routing-decisions", limit, offset],
    queryFn: () => getRoutingDecisions(limit, offset),
    staleTime: 5_000,
    // Live feed: refetch every 5s so operators see decisions in near-real-time
    refetchInterval: 5_000,
  });
}

export function useToggleRouting() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (enabled: boolean) => toggleRouting(enabled),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["routing-policy"] });
    },
  });
}

export function useDryRun() {
  return useMutation({
    mutationFn: ({ start, end, limit }: { start: string; end: string; limit?: number }) =>
      postDryRun(start, end, limit),
  });
}
