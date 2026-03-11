import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createDebugExperiment,
  createDebugHypothesis,
  createDebugRun,
  createDebugSession,
  getDebugSession,
  getDebugSessions,
} from "../api/client";
import type {
  CreateExperimentRequest,
  CreateDebugSessionRequest,
  CreateHypothesisRequest,
  CreateRunRequest,
  DebugSessionDetail,
  DebugSessionSummary,
} from "../api/types";

export function useDebugSessions() {
  return useQuery<DebugSessionSummary[]>({
    queryKey: ["debug-sessions"],
    queryFn: () => getDebugSessions(),
    staleTime: 15_000,
  });
}

export function useDebugSession(sessionId?: string) {
  return useQuery<DebugSessionDetail>({
    queryKey: ["debug-session", sessionId],
    queryFn: () => getDebugSession(sessionId as string),
    enabled: Boolean(sessionId),
    staleTime: 10_000,
  });
}

export function useCreateDebugSession() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateDebugSessionRequest) => createDebugSession(body),
    onSuccess: (session) => {
      queryClient.invalidateQueries({ queryKey: ["debug-sessions"] });
      queryClient.setQueryData(["debug-session", session.id], {
        session,
        hypotheses: [],
        experiments: [],
        runs: [],
      });
    },
  });
}

export function useCreateDebugHypothesis(sessionId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateHypothesisRequest) =>
      createDebugHypothesis(sessionId, body),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["debug-session", sessionId] });
    },
  });
}

export function useCreateDebugExperiment(sessionId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateExperimentRequest) =>
      createDebugExperiment(sessionId, body),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["debug-session", sessionId] });
    },
  });
}

export function useCreateDebugRun(sessionId: string, experimentId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateRunRequest) => createDebugRun(experimentId, body),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["debug-session", sessionId] });
    },
  });
}
