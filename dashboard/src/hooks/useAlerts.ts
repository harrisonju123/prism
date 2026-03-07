import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
  getAlertRules,
  createAlertRule,
  deleteAlertRule,
  getAlertHistory,
  getBudgets,
  createBudget,
} from "../api/client";
import type { AlertRuleCreate, BudgetCreate } from "../api/types";

export function useAlertRules() {
  return useQuery({
    queryKey: ["alert-rules"],
    queryFn: getAlertRules,
    staleTime: 30_000,
  });
}

export function useCreateAlertRule() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: AlertRuleCreate) => createAlertRule(body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["alert-rules"] }),
  });
}

export function useDeleteAlertRule() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (ruleId: string) => deleteAlertRule(ruleId),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["alert-rules"] }),
  });
}

export function useAlertHistory(limit = 50, offset = 0) {
  return useQuery({
    queryKey: ["alert-history", limit, offset],
    queryFn: () => getAlertHistory(limit, offset),
    staleTime: 30_000,
  });
}

export function useBudgets() {
  return useQuery({
    queryKey: ["budgets"],
    queryFn: getBudgets,
    staleTime: 30_000,
  });
}

export function useCreateBudget() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: BudgetCreate) => createBudget(body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["budgets"] }),
  });
}
