import { useState } from "react";
import {
  useAlertRules,
  useCreateAlertRule,
  useDeleteAlertRule,
  useAlertHistory,
  useBudgets,
  useCreateBudget,
} from "../hooks/useAlerts";
import { CardShell } from "../components/common/CardShell";
import { InfoTip } from "../components/common/InfoTip";
import { formatUSD } from "../utils/format";
import type {
  AlertRuleCreate,
  BudgetCreate,
  RuleType,
  AlertChannel,
  BudgetPeriod,
  BudgetAction,
} from "../api/types";

const RULE_TYPES: RuleType[] = ["spend_threshold", "anomaly_zscore", "error_rate", "latency_p95"];
const CHANNELS: AlertChannel[] = ["slack", "email", "both"];
const BUDGET_PERIODS: BudgetPeriod[] = ["daily", "weekly", "monthly"];
const BUDGET_ACTIONS: BudgetAction[] = ["alert", "downgrade", "block"];

export function Alerts() {
  const [showRuleForm, setShowRuleForm] = useState(false);
  const [showBudgetForm, setShowBudgetForm] = useState(false);

  return (
    <div className="flex flex-col gap-6 max-w-[1600px] mx-auto">
      <h1 className="text-lg font-semibold flex items-center gap-2">Alerts & Budgets <InfoTip text="Configure spend thresholds, anomaly detection, and budget caps. Alerts fire to Slack or email when conditions are met." /></h1>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Alert Rules */}
        <div className="flex flex-col gap-3">
          <div className="flex items-center justify-between">
            <h2 className="text-sm font-medium text-gray-300 flex items-center gap-1">Alert Rules <InfoTip text="Rules evaluate periodically against recent metrics. Types: spend_threshold, anomaly_zscore, error_rate, latency_p95." /></h2>
            <button
              onClick={() => setShowRuleForm((v) => !v)}
              className="px-2.5 py-1 text-xs bg-violet-600/20 text-violet-300 rounded hover:bg-violet-600/30 transition-colors"
            >
              {showRuleForm ? "Cancel" : "+ Rule"}
            </button>
          </div>
          {showRuleForm && <AlertRuleForm onDone={() => setShowRuleForm(false)} />}
          <AlertRulesList />
        </div>

        {/* Budgets */}
        <div className="flex flex-col gap-3">
          <div className="flex items-center justify-between">
            <h2 className="text-sm font-medium text-gray-300 flex items-center gap-1">Budgets <InfoTip text="Spending caps with automatic enforcement. Actions: alert (notify only), downgrade (switch to cheaper models), block (reject requests)." /></h2>
            <button
              onClick={() => setShowBudgetForm((v) => !v)}
              className="px-2.5 py-1 text-xs bg-violet-600/20 text-violet-300 rounded hover:bg-violet-600/30 transition-colors"
            >
              {showBudgetForm ? "Cancel" : "+ Budget"}
            </button>
          </div>
          {showBudgetForm && <BudgetForm onDone={() => setShowBudgetForm(false)} />}
          <BudgetsList />
        </div>
      </div>

      {/* Alert History */}
      <AlertHistorySection />
    </div>
  );
}

// -- Alert Rules List ---------------------------------------------------------

function AlertRulesList() {
  const { data: rules, isLoading, error } = useAlertRules();
  const deleteMutation = useDeleteAlertRule();
  const [deletingId, setDeletingId] = useState<string | null>(null);

  return (
    <CardShell loading={isLoading} error={error ?? null} skeletonHeight="h-32">
      {rules?.length === 0 && (
        <p className="text-xs text-gray-500 py-4 text-center">No alert rules configured</p>
      )}
      <div className="flex flex-col gap-2">
        {rules?.map((rule) => (
          <div
            key={rule.id}
            className="flex items-center justify-between bg-gray-800/50 rounded px-3 py-2"
          >
            <div className="flex flex-col gap-0.5">
              <div className="flex items-center gap-2">
                <span className="text-xs font-mono text-gray-200">{rule.rule_type}</span>
                <span className={`text-[10px] px-1.5 py-0.5 rounded ${
                  rule.enabled
                    ? "bg-green-500/15 text-green-400"
                    : "bg-gray-700 text-gray-500"
                }`}>
                  {rule.enabled ? "active" : "disabled"}
                </span>
              </div>
              <span className="text-[10px] text-gray-500">
                {rule.channel} · {rule.org_id}
              </span>
            </div>
            <button
              onClick={() => {
                setDeletingId(rule.id);
                deleteMutation.mutate(rule.id, { onSettled: () => setDeletingId(null) });
              }}
              disabled={deletingId === rule.id}
              className="text-gray-500 hover:text-red-400 text-xs transition-colors disabled:opacity-40"
            >
              Delete
            </button>
          </div>
        ))}
      </div>
    </CardShell>
  );
}

// -- Alert Rule Form ----------------------------------------------------------

function AlertRuleForm({ onDone }: { onDone: () => void }) {
  const mutation = useCreateAlertRule();
  const [ruleType, setRuleType] = useState<RuleType>("spend_threshold");
  const [channel, setChannel] = useState<AlertChannel>("slack");
  const [threshold, setThreshold] = useState("100");
  const [webhookUrl, setWebhookUrl] = useState("");
  const [orgId, setOrgId] = useState("default");

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const body: AlertRuleCreate = {
      org_id: orgId,
      rule_type: ruleType,
      threshold_config: { value: Number(threshold) },
      channel,
      webhook_url: webhookUrl || null,
    };
    mutation.mutate(body, { onSuccess: onDone });
  }

  return (
    <form onSubmit={handleSubmit} className="bg-gray-800/50 rounded p-3 flex flex-col gap-2">
      <div className="grid grid-cols-2 gap-2">
        <label className="flex flex-col gap-1">
          <span className="text-[10px] text-gray-500 uppercase">Type</span>
          <select value={ruleType} onChange={(e) => setRuleType(e.target.value as RuleType)}
            className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200">
            {RULE_TYPES.map((t) => <option key={t} value={t}>{t}</option>)}
          </select>
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-[10px] text-gray-500 uppercase">Channel</span>
          <select value={channel} onChange={(e) => setChannel(e.target.value as AlertChannel)}
            className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200">
            {CHANNELS.map((c) => <option key={c} value={c}>{c}</option>)}
          </select>
        </label>
      </div>
      <div className="grid grid-cols-2 gap-2">
        <label className="flex flex-col gap-1">
          <span className="text-[10px] text-gray-500 uppercase">Threshold</span>
          <input type="number" value={threshold} onChange={(e) => setThreshold(e.target.value)}
            className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200" />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-[10px] text-gray-500 uppercase">Org ID</span>
          <input type="text" value={orgId} onChange={(e) => setOrgId(e.target.value)}
            className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200" />
        </label>
      </div>
      {(channel === "slack" || channel === "both") && (
        <label className="flex flex-col gap-1">
          <span className="text-[10px] text-gray-500 uppercase">Webhook URL</span>
          <input type="url" value={webhookUrl} onChange={(e) => setWebhookUrl(e.target.value)}
            placeholder="https://hooks.slack.com/..."
            className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200 placeholder:text-gray-600" />
        </label>
      )}
      {mutation.isError && (
        <p className="text-xs text-red-400">{(mutation.error as Error).message}</p>
      )}
      <button type="submit" disabled={mutation.isPending}
        className="self-end px-3 py-1.5 text-xs bg-violet-600 text-white rounded hover:bg-violet-500 disabled:opacity-50 transition-colors">
        Create Rule
      </button>
    </form>
  );
}

// -- Budgets List -------------------------------------------------------------

function BudgetsList() {
  const { data: budgets, isLoading, error } = useBudgets();

  return (
    <CardShell loading={isLoading} error={error ?? null} skeletonHeight="h-32">
      {budgets?.length === 0 && (
        <p className="text-xs text-gray-500 py-4 text-center">No budgets configured</p>
      )}
      <div className="flex flex-col gap-2">
        {budgets?.map((budget) => {
          const pct = budget.budget_usd > 0
            ? (budget.current_spend / budget.budget_usd) * 100
            : 0;
          return (
            <div key={budget.id} className="bg-gray-800/50 rounded px-3 py-2">
              <div className="flex items-center justify-between mb-1">
                <span className="text-xs font-mono text-gray-200">
                  {formatUSD(budget.budget_usd)} / {budget.period}
                </span>
                <span className={`text-[10px] px-1.5 py-0.5 rounded ${
                  budget.action === "block" ? "bg-red-500/15 text-red-400"
                    : budget.action === "downgrade" ? "bg-yellow-500/15 text-yellow-400"
                    : "bg-blue-500/15 text-blue-400"
                }`}>
                  {budget.action}
                </span>
              </div>
              {/* Utilization bar */}
              <div className="h-1.5 bg-gray-700 rounded-full overflow-hidden">
                <div
                  className={`h-full rounded-full transition-all ${
                    pct > 90 ? "bg-red-500" : pct > 70 ? "bg-yellow-500" : "bg-violet-500"
                  }`}
                  style={{ width: `${Math.min(pct, 100)}%` }}
                />
              </div>
              <div className="flex justify-between mt-1">
                <span className="text-[10px] text-gray-500">{formatUSD(budget.current_spend)} spent</span>
                <span className="text-[10px] text-gray-500">{pct.toFixed(1)}%</span>
              </div>
            </div>
          );
        })}
      </div>
    </CardShell>
  );
}

// -- Budget Form --------------------------------------------------------------

function BudgetForm({ onDone }: { onDone: () => void }) {
  const mutation = useCreateBudget();
  const [amount, setAmount] = useState("100");
  const [period, setPeriod] = useState<BudgetPeriod>("daily");
  const [action, setAction] = useState<BudgetAction>("alert");
  const [orgId, setOrgId] = useState("default");

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const body: BudgetCreate = {
      org_id: orgId,
      budget_usd: Number(amount),
      period,
      action,
    };
    mutation.mutate(body, { onSuccess: onDone });
  }

  return (
    <form onSubmit={handleSubmit} className="bg-gray-800/50 rounded p-3 flex flex-col gap-2">
      <div className="grid grid-cols-3 gap-2">
        <label className="flex flex-col gap-1">
          <span className="text-[10px] text-gray-500 uppercase">Amount (USD)</span>
          <input type="number" value={amount} onChange={(e) => setAmount(e.target.value)}
            className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200" />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-[10px] text-gray-500 uppercase">Period</span>
          <select value={period} onChange={(e) => setPeriod(e.target.value as BudgetPeriod)}
            className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200">
            {BUDGET_PERIODS.map((p) => <option key={p} value={p}>{p}</option>)}
          </select>
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-[10px] text-gray-500 uppercase">Action</span>
          <select value={action} onChange={(e) => setAction(e.target.value as BudgetAction)}
            className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200">
            {BUDGET_ACTIONS.map((a) => <option key={a} value={a}>{a}</option>)}
          </select>
        </label>
      </div>
      <label className="flex flex-col gap-1">
        <span className="text-[10px] text-gray-500 uppercase">Org ID</span>
        <input type="text" value={orgId} onChange={(e) => setOrgId(e.target.value)}
          className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200" />
      </label>
      {mutation.isError && (
        <p className="text-xs text-red-400">{(mutation.error as Error).message}</p>
      )}
      <button type="submit" disabled={mutation.isPending}
        className="self-end px-3 py-1.5 text-xs bg-violet-600 text-white rounded hover:bg-violet-500 disabled:opacity-50 transition-colors">
        Create Budget
      </button>
    </form>
  );
}

// -- Alert History ------------------------------------------------------------

function AlertHistorySection() {
  const { data, isLoading, error } = useAlertHistory();

  return (
    <div className="flex flex-col gap-3">
      <h2 className="text-sm font-medium text-gray-300">Alert History</h2>
      <CardShell loading={isLoading} error={error ?? null} skeletonHeight="h-48">
        {data?.items.length === 0 && (
          <p className="text-xs text-gray-500 py-4 text-center">No alerts fired yet</p>
        )}
        <div className="flex flex-col gap-1">
          {data?.items.map((item) => (
            <div key={item.id} className="flex items-center gap-3 px-3 py-2 bg-gray-800/30 rounded">
              <span className={`text-[10px] px-1.5 py-0.5 rounded font-medium ${
                item.severity === "critical" ? "bg-red-500/15 text-red-400"
                  : item.severity === "warning" ? "bg-yellow-500/15 text-yellow-400"
                  : "bg-blue-500/15 text-blue-400"
              }`}>
                {item.severity}
              </span>
              <span className="text-xs text-gray-200 flex-1">{item.message}</span>
              <span className="text-[10px] text-gray-500 whitespace-nowrap">
                {new Date(item.triggered_at).toLocaleString("en-US", {
                  month: "short", day: "numeric", hour: "numeric", minute: "2-digit",
                })}
              </span>
              {item.resolved && (
                <span className="text-[10px] text-green-400">resolved</span>
              )}
            </div>
          ))}
        </div>
      </CardShell>
    </div>
  );
}
