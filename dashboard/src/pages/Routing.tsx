import { useState } from "react";
import {
  useRoutingPolicy,
  useRoutingDecisions,
  useDryRun,
  useToggleRouting,
} from "../hooks/useRouting";
import { CardShell } from "../components/common/CardShell";
import { InfoTip } from "../components/common/InfoTip";
import { formatMs, formatUSD } from "../utils/format";

export function Routing() {
  const { data: policyData } = useRoutingPolicy();
  const toggleMutation = useToggleRouting();
  const routingEnabled = policyData?.routing_enabled ?? false;

  return (
    <div className="flex flex-col gap-6 max-w-[1600px] mx-auto">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold flex items-center gap-2">Routing <InfoTip text="Automatic model selection based on task type, quality requirements, and cost constraints." /></h1>
        <button
          type="button"
          role="switch"
          aria-checked={routingEnabled}
          disabled={toggleMutation.isPending}
          onClick={() => toggleMutation.mutate(!routingEnabled)}
          className={[
            "relative inline-flex h-6 w-11 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out disabled:opacity-50",
            routingEnabled ? "bg-green-500" : "bg-gray-600",
          ].join(" ")}
        >
          <span
            className={[
              "pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out",
              routingEnabled ? "translate-x-5" : "translate-x-0",
            ].join(" ")}
          />
        </button>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <PolicyViewer />
        <DryRunSection />
      </div>

      <DecisionsFeed />
    </div>
  );
}

// -- Policy Viewer ------------------------------------------------------------

function PolicyViewer() {
  const { data, isLoading, error } = useRoutingPolicy();
  const policy = data?.policy;

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-medium text-gray-300 flex items-center gap-1">Active Policy <InfoTip text="Current routing rules. Each maps a task type to quality/cost/latency constraints. The router picks the cheapest qualifying model." /></h2>
        {data && (
          <span className="text-[10px] text-gray-500 font-mono">
            v{policy?.version}{data.is_default ? " (default)" : ""}
          </span>
        )}
      </div>
      <CardShell loading={isLoading} error={error ?? null} skeletonHeight="h-48">
        {policy?.rules.length === 0 && (
          <p className="text-xs text-gray-500 py-4 text-center">No routing rules configured</p>
        )}
        {policy && policy.rules.length > 0 && (
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="text-left text-[10px] text-gray-500 uppercase tracking-wider">
                  <th className="pb-2 pr-3 font-medium">Task Type</th>
                  <th className="pb-2 pr-3 font-medium">Criteria</th>
                  <th className="pb-2 pr-3 font-medium text-right"><span className="inline-flex items-center gap-1">Min Quality <InfoTip text="Minimum quality score (0–1) from the fitness matrix. Models below this are excluded." /></span></th>
                  <th className="pb-2 pr-3 font-medium text-right"><span className="inline-flex items-center gap-1">Max Cost/1k <InfoTip text="Maximum cost per 1,000 tokens. Models exceeding this are excluded." /></span></th>
                  <th className="pb-2 pr-3 font-medium text-right">Max Latency</th>
                  <th className="pb-2 font-medium"><span className="inline-flex items-center gap-1">Fallback <InfoTip text="Model used when no candidate meets all constraints." /></span></th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-800">
                {policy.rules.map((rule, i) => (
                  <tr key={i} className="text-gray-200">
                    <td className="py-2 pr-3">
                      <span className="font-mono bg-gray-800 px-1.5 py-0.5 rounded text-violet-300">
                        {rule.task_type}
                      </span>
                    </td>
                    <td className="py-2 pr-3 text-gray-400">{rule.criteria}</td>
                    <td className="py-2 pr-3 text-right font-mono">{rule.min_quality.toFixed(2)}</td>
                    <td className="py-2 pr-3 text-right font-mono">{rule.max_cost_per_1k != null ? formatUSD(rule.max_cost_per_1k) : "—"}</td>
                    <td className="py-2 pr-3 text-right font-mono">{rule.max_latency_ms != null ? formatMs(rule.max_latency_ms) : "—"}</td>
                    <td className="py-2 font-mono text-gray-400">{rule.fallback}</td>
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

// -- Decisions Feed -----------------------------------------------------------

function DecisionsFeed() {
  const { data, isLoading, error } = useRoutingDecisions();

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-medium text-gray-300">Recent Decisions</h2>
        <span className="text-[10px] text-gray-500 flex items-center gap-1.5">
          {/* Pulsing dot to indicate live polling */}
          <span className="relative flex h-2 w-2">
            <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-violet-400 opacity-75" />
            <span className="relative inline-flex rounded-full h-2 w-2 bg-violet-500" />
          </span>
          live
        </span>
      </div>
      <CardShell loading={isLoading} error={error ?? null} skeletonHeight="h-64">
        {data?.decisions.length === 0 && (
          <p className="text-xs text-gray-500 py-4 text-center">No routing decisions recorded yet</p>
        )}
        {data && data.decisions.length > 0 && (
          <div className="flex flex-col gap-1 max-h-[480px] overflow-y-auto">
            {data.decisions.map((d, i) => (
              <div
                key={i}
                className={[
                  "flex items-center gap-3 px-3 py-2 rounded",
                  d.was_overridden
                    ? "bg-yellow-500/8 border border-yellow-500/20"
                    : "bg-gray-800/30",
                ].join(" ")}
              >
                {/* Model badge */}
                <span className="text-xs font-mono text-gray-200 min-w-[140px] truncate">
                  {d.selected_model}
                </span>

                {/* Group tag */}
                {d.group_name && (
                  <span className="text-[10px] font-mono bg-gray-800 text-gray-400 px-1.5 py-0.5 rounded shrink-0">
                    {d.group_name}
                  </span>
                )}

                {/* Override indicator */}
                {d.was_overridden && (
                  <span className="text-[10px] px-1.5 py-0.5 rounded bg-yellow-500/15 text-yellow-400 shrink-0">
                    overridden
                  </span>
                )}

                {/* Reason */}
                <span className="text-xs text-gray-400 flex-1 truncate">{d.reason}</span>

                {/* Policy version reference */}
                {d.policy_version != null && (
                  <span className="text-[10px] text-gray-600 font-mono shrink-0">
                    v{d.policy_version}
                  </span>
                )}
              </div>
            ))}
          </div>
        )}
        {data && (
          <div className="mt-2 pt-2 border-t border-gray-800 flex justify-between">
            <span className="text-[10px] text-gray-500">
              Showing {data.decisions.length} of {data.total_count}
            </span>
          </div>
        )}
      </CardShell>
    </div>
  );
}

// -- Dry Run ------------------------------------------------------------------

function DryRunSection() {
  const mutation = useDryRun();
  const [start, setStart] = useState(() => {
    const d = new Date();
    d.setHours(d.getHours() - 24);
    return d.toISOString().slice(0, 16);
  });
  const [end, setEnd] = useState(() => new Date().toISOString().slice(0, 16));

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    mutation.mutate({
      start: new Date(start).toISOString(),
      end: new Date(end).toISOString(),
    });
  }

  return (
    <div className="flex flex-col gap-3">
      <h2 className="text-sm font-medium text-gray-300 flex items-center gap-1">Dry Run <InfoTip text="Replays historical traffic through the current policy to show what the router would have chosen vs. what actually happened." /></h2>
      <CardShell>
        <form onSubmit={handleSubmit} className="flex flex-col gap-3">
          <p className="text-[10px] text-gray-500">
            Simulate routing decisions over historical traffic without applying changes.
          </p>
          <div className="grid grid-cols-2 gap-2">
            <label className="flex flex-col gap-1">
              <span className="text-[10px] text-gray-500 uppercase">Start</span>
              <input
                type="datetime-local"
                value={start}
                onChange={(e) => setStart(e.target.value)}
                className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200"
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className="text-[10px] text-gray-500 uppercase">End</span>
              <input
                type="datetime-local"
                value={end}
                onChange={(e) => setEnd(e.target.value)}
                className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200"
              />
            </label>
          </div>
          <button
            type="submit"
            disabled={mutation.isPending}
            className="self-end px-3 py-1.5 text-xs bg-violet-600 text-white rounded hover:bg-violet-500 disabled:opacity-50 transition-colors flex items-center gap-1.5"
          >
            {mutation.isPending && (
              <svg className="w-3 h-3 animate-spin" viewBox="0 0 16 16" fill="none">
                <circle cx="8" cy="8" r="6" stroke="currentColor" strokeWidth="2" strokeDasharray="28" strokeDashoffset="8" strokeLinecap="round" />
              </svg>
            )}
            Run Simulation
          </button>
        </form>

        {mutation.isError && (
          <p className="text-xs text-red-400 mt-2">{(mutation.error as Error).message}</p>
        )}

        {mutation.isSuccess && mutation.data && (
          <DryRunResults data={mutation.data} />
        )}
      </CardShell>
    </div>
  );
}

function DryRunResults({ data }: { data: Record<string, unknown> }) {
  // The dry-run API returns a flexible shape; render key-value pairs
  // for top-level fields plus nested tables for arrays.
  const entries = Object.entries(data);

  if (entries.length === 0) {
    return (
      <p className="text-xs text-gray-500 mt-3 text-center">No results returned</p>
    );
  }

  return (
    <div className="mt-3 pt-3 border-t border-gray-800 flex flex-col gap-2">
      <h3 className="text-[10px] text-gray-500 uppercase tracking-wider">Results</h3>
      {entries.map(([key, value]) => {
        if (Array.isArray(value)) {
          return (
            <div key={key} className="flex flex-col gap-1">
              <span className="text-[10px] text-gray-400 font-mono">{key}</span>
              <div className="max-h-60 overflow-y-auto">
                <table className="w-full text-[11px]">
                  <thead>
                    <tr className="text-left text-[10px] text-gray-500 uppercase">
                      {value.length > 0 &&
                        Object.keys(value[0] as Record<string, unknown>).map((col) => (
                          <th key={col} className="pb-1 pr-2 font-medium">{col}</th>
                        ))}
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-gray-800/50">
                    {value.map((row, ri) => (
                      <tr key={ri}>
                        {Object.values(row as Record<string, unknown>).map((cell, ci) => (
                          <td key={ci} className="py-1 pr-2 text-gray-300 font-mono">
                            {String(cell)}
                          </td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          );
        }
        return (
          <div key={key} className="flex items-center gap-2">
            <span className="text-[10px] text-gray-400 font-mono">{key}:</span>
            <span className="text-xs text-gray-200 font-mono">
              {typeof value === "object" ? JSON.stringify(value) : String(value)}
            </span>
          </div>
        );
      })}
    </div>
  );
}
