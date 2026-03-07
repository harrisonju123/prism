import { useQuery } from "@tanstack/react-query";
import { getEvents, getEventById } from "../api/client";
import { rangeToParams, type TimeRange } from "./useStats";

export interface EventFilters {
  model?: string;
  status?: "success" | "failure";
  task_type?: string;
  limit?: number;
  offset?: number;
}

export function useEvents(range: TimeRange, filters: EventFilters = {}) {
  const { start, end } = rangeToParams(range);

  const params: Record<string, string> = { start, end };
  if (filters.model) params.model = filters.model;
  if (filters.status) params.status = filters.status;
  if (filters.task_type) params.task_type = filters.task_type;
  if (filters.limit != null) params.limit = String(filters.limit);
  if (filters.offset != null) params.offset = String(filters.offset);

  return useQuery({
    queryKey: ["events", range, filters],
    queryFn: () => getEvents(params),
    staleTime: 30_000,
  });
}

/** Fetch a single event by ID. Only enabled when eventId is non-null. */
export function useEvent(eventId: string | null) {
  return useQuery({
    queryKey: ["event", eventId],
    queryFn: () => getEventById(eventId!),
    enabled: eventId != null,
    staleTime: 60_000,
  });
}
