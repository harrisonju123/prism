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
  DebugExperiment,
  DebugHypothesis,
  DebugRun,
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
  return useMutation<DebugHypothesis, Error, CreateHypothesisRequest>({
    mutationFn: (body) => createDebugHypothesis(sessionId, body),
    onSuccess: (hypothesis) => {
      queryClient.setQueryData<DebugSessionDetail>(
        ["debug-session", sessionId],
        (prev) =>
          prev
            ? { ...prev, hypotheses: [...prev.hypotheses, hypothesis] }
            : prev,
      );
    },
  });
}

export function useCreateDebugExperiment(sessionId: string) {
  const queryClient = useQueryClient();
  return useMutation<DebugExperiment, Error, CreateExperimentRequest>({
    mutationFn: (body) => createDebugExperiment(sessionId, body),
    onSuccess: (experiment) => {
      queryClient.setQueryData<DebugSessionDetail>(
        ["debug-session", sessionId],
        (prev) =>
          prev
            ? { ...prev, experiments: [...prev.experiments, experiment] }
            : prev,
      );
    },
  });
}

export function useCreateDebugRun(sessionId: string, experimentId: string) {
  const queryClient = useQueryClient();
  return useMutation<DebugRun, Error, CreateRunRequest>({
    mutationFn: (body) => createDebugRun(experimentId, body),
    onSuccess: (run) => {
      queryClient.setQueryData<DebugSessionDetail>(
        ["debug-session", sessionId],
        (prev) => (prev ? { ...prev, runs: [...prev.runs, run] } : prev),
      );
    },
  });
}
